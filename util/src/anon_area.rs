use super::PAGE_SIZE;
use num::Integer;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;

std::thread_local!(static RNG: RefCell<SmallRng> = RefCell::new(SmallRng::from_entropy()));

struct AnonUnit {
    data: *mut u8,
    layout: Layout,
}

impl AnonUnit {
    fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, *PAGE_SIZE).unwrap();
        Self {
            data: unsafe { alloc(layout) },
            layout,
        }
    }
}

unsafe impl Send for AnonUnit {}
unsafe impl Sync for AnonUnit {}

impl Drop for AnonUnit {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.data, self.layout);
        }
    }
}

pub struct AnonArea {
    units: Vec<AnonUnit>,
    size: usize,
    comp: f64,
}

/// Anonymous memory which can be shared by multiple threads with RwLock
/// protection. Accesses to memory positions only require read locking for both
/// reads and writes.
impl AnonArea {
    const UNIT_SIZE: usize = 32 << 20;

    pub fn new(size: usize, comp: f64) -> Self {
        let mut area = AnonArea {
            units: Vec::new(),
            size: 0,
            comp,
        };
        area.resize(size);
        area
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn resize(&mut self, mut size: usize) {
        size = size.max(Self::UNIT_SIZE);
        let nr = size.div_ceil(&Self::UNIT_SIZE);

        self.units.truncate(nr);
        self.units.reserve(nr);
        for _ in self.units.len()..nr {
            self.units.push(AnonUnit::new(Self::UNIT_SIZE));
        }

        self.size = size;
    }

    /// Determine the page given the relative position `rel` and `size` of
    /// the anon area. `rel` is in the range [-1.0, 1.0] with the position
    /// 0.0 mapping to the first page, positive positions to even slots and
    /// negative odd so that modulating the amplitude of `rel` changes how
    /// much area is accessed without shifting the center.
    pub fn rel_to_page_idx(rel: f64, size: usize) -> usize {
        let addr = ((size / 2) as f64 * rel.abs()) as usize;
        let mut page_idx = (addr / *PAGE_SIZE) * 2;
        if rel.is_sign_negative() {
            page_idx += 1;
        }
        page_idx.min(size / *PAGE_SIZE - 1)
    }

    /// Return a mutable u8 reference to the position specified by the page
    /// index. The anon area is shared and there's no access control.
    pub fn access_page<'a, T>(&'a self, page_idx: usize) -> &'a mut [T] {
        let pages_per_unit = Self::UNIT_SIZE / *PAGE_SIZE;
        let pos = (
            page_idx / pages_per_unit,
            (page_idx % pages_per_unit) * *PAGE_SIZE,
        );
        unsafe {
            let ptr = self.units[pos.0].data.offset(pos.1 as isize);
            let ptr = ptr.cast::<T>();
            std::slice::from_raw_parts_mut(ptr, *PAGE_SIZE / std::mem::size_of::<T>())
        }
    }

    pub fn fill_page_with_random(&self, page_idx: usize) {
        RNG.with(|s| {
            super::fill_area_with_random(
                self.access_page::<u8>(page_idx),
                self.comp,
                &mut *s.borrow_mut(),
            )
        });
    }
}
