use super::*;
use plotlib::page::Page;
use plotlib::repr::Plot;
use plotlib::style::{LineStyle, PointMarker, PointStyle};
use plotlib::view::ContinuousView;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Grapher<'a, 'b> {
    vrate_range: (f64, f64),
    data: &'a JobData,
    res: &'b IoCostTuneResult,
}

impl<'a, 'b> Grapher<'a, 'b> {
    pub fn new(vrate_range: (f64, f64), data: &'a JobData, res: &'b IoCostTuneResult) -> Self {
        Self {
            vrate_range,
            data,
            res,
        }
    }

    fn setup_view(
        vrate_range: (f64, f64),
        sel: &DataSel,
        series: &DataSeries,
        mem_profile: u32,
        isol_pct: &str,
        extra_info: Option<&str>,
    ) -> (ContinuousView, f64) {
        let (val_min, val_max) = series
            .data
            .iter()
            .chain(series.outliers.iter())
            .fold((std::f64::MAX, 0.0_f64), |acc, point| {
                (acc.0.min(point.y), acc.1.max(point.y))
            });

        let (ymin, yscale) = match sel {
            DataSel::MOF => {
                let ymin = if val_min >= 1.0 {
                    1.0
                } else {
                    val_min - (val_max - val_min) / 10.0
                };
                (ymin, 1.0)
            }
            DataSel::AMOF => {
                let ymin = if val_min >= 1.0 {
                    1.0
                } else {
                    val_min - (val_max - val_min) / 10.0
                };
                (ymin, 1.0)
            }
            DataSel::AMOFDelta => (0.0, 1.0),
            DataSel::Isol => (0.0, 100.0),
            DataSel::IsolPct(_) => (0.0, 100.0),
            DataSel::IsolMean => (0.0, 100.0),
            DataSel::LatImp => (0.0, 100.0),
            DataSel::WorkCsv => (0.0, 100.0),
            DataSel::Missing => (0.0, 100.0),
            DataSel::RLat(_, _) => (0.0, 1000.0),
            DataSel::WLat(_, _) => (0.0, 1000.0),
        };
        let ymax = (val_max * 1.1).max((ymin) + 0.000001);

        let range = series.lines.range;
        let mut xlabel = format!("vrate {:.1}-{:.1} (", range.0, range.1);

        let (min, max) = series.lines.min_max();
        if min == max {
            xlabel += &format!("mean={:.3}", min * yscale);
        } else {
            xlabel += &format!("min={:.3} max={:.3}", min * yscale, max * yscale);
        }

        let points = &series.lines.points;
        if points.len() > 2 {
            xlabel += " infls=";
            for i in 1..points.len() - 1 {
                xlabel += &format!("{:.1},", points[i].x);
            }
            xlabel.pop();
        }
        xlabel += &format!(" err={:.3})", series.error * yscale);

        let mut ylabel = match sel {
            DataSel::MOF | DataSel::AMOF | DataSel::AMOFDelta => format!("{}@{}", sel, mem_profile),
            DataSel::Isol => format!("isol-{}", isol_pct),
            sel => format!("{}", sel),
        };
        if extra_info.is_some() {
            ylabel += &format!(" ({})", extra_info.as_ref().unwrap());
        }

        let view = ContinuousView::new()
            .x_range(0.0, (vrate_range.1 * 1.1).max(0.000001))
            .y_range(ymin * yscale, ymax * yscale)
            .x_label(xlabel)
            .y_label(ylabel);

        (view, yscale)
    }

    fn plot_one_text<'o>(
        &mut self,
        out: &mut Box<dyn Write + 'o>,
        sel: &DataSel,
        series: &DataSeries,
        mem_profile: u32,
        isol_pct: &str,
    ) -> Result<()> {
        const SIZE: (u32, u32) = (80, 24);
        let (view, yscale) =
            Self::setup_view(self.vrate_range, sel, series, mem_profile, isol_pct, None);

        let mut lines = vec![];
        for i in 0..SIZE.0 {
            let vrate = series.lines.range.1 / SIZE.0 as f64 * i as f64;
            if vrate >= series.lines.range.0 {
                lines.push((vrate, series.lines.eval(vrate) * yscale));
            }
        }
        let view =
            view.add(Plot::new(lines).point_style(PointStyle::new().marker(PointMarker::Square)));

        let outliers = series
            .outliers
            .iter()
            .map(|p| (p.x, p.y * yscale))
            .collect();
        let view =
            view.add(Plot::new(outliers).point_style(PointStyle::new().marker(PointMarker::Cross)));

        let points = series.data.iter().map(|p| (p.x, p.y * yscale)).collect();
        let view =
            view.add(Plot::new(points).point_style(PointStyle::new().marker(PointMarker::Circle)));

        let page = Page::single(&view).dimensions(SIZE.0, SIZE.1);
        write!(out, "{}\n\n", page.to_text().unwrap()).unwrap();
        Ok(())
    }

    fn plot_one_svg(
        &mut self,
        dir: &Path,
        sel: &DataSel,
        series: &DataSeries,
        mem_profile: u32,
        isol_pct: &str,
        extra_info: &str,
    ) -> Result<()> {
        const SIZE: (u32, u32) = (576, 468);
        let (mut view, yscale) = Self::setup_view(
            self.vrate_range,
            sel,
            series,
            mem_profile,
            isol_pct,
            Some(extra_info),
        );

        let points = series
            .outliers
            .iter()
            .map(|p| (p.x, p.y * yscale))
            .collect();
        view = view.add(
            Plot::new(points).point_style(
                PointStyle::new()
                    .marker(PointMarker::Cross)
                    .colour("#37c0e6"),
            ),
        );

        let points = series.data.iter().map(|p| (p.x, p.y * yscale)).collect();
        view = view.add(
            Plot::new(points).point_style(
                PointStyle::new()
                    .marker(PointMarker::Circle)
                    .colour("#37c0e6"),
            ),
        );

        let segments: Vec<(f64, f64)> = series
            .lines
            .points
            .iter()
            .map(|pt| (pt.x, pt.y * yscale))
            .collect();
        if !segments.is_empty() {
            view = view.add(Plot::new(segments).line_style(LineStyle::new().colour("#3749e6")));
        }

        view = view.x_max_ticks(10).y_max_ticks(10);

        let mut path = PathBuf::from(dir);
        path.push(format!("iocost-tune-{}.svg", sel));
        if let Err(e) = Page::single(&view).dimensions(SIZE.0, SIZE.1).save(&path) {
            bail!("{}", &e);
        }
        Ok(())
    }

    fn collect_svgs(dir: &Path, sels: Vec<DataSel>, dst: &Path) -> Result<()> {
        const NR_PER_PAGE: usize = 6;

        let groups = DataSel::align_and_merge_groups(DataSel::group(sels), NR_PER_PAGE);
        let mut srcs: Vec<PathBuf> = vec![];
        for grp in groups.iter() {
            srcs.extend(grp.iter().map(|sel| {
                let mut src = PathBuf::from(dir);
                src.push(format!("iocost-tune-{}.svg", sel));
                src
            }));
            let pad = NR_PER_PAGE - (grp.len() % NR_PER_PAGE);
            if pad < NR_PER_PAGE {
                srcs.extend(std::iter::repeat(PathBuf::from("null:")).take(pad));
            }
        }

        run_command(
            Command::new("montage")
                .args(&[
                    "-font",
                    "Source-Code-Pro",
                    "-density",
                    "300",
                    "-tile",
                    "2x3",
                    "-geometry",
                    "+0+0",
                ])
                .args(srcs)
                .arg(dst),
            "Are imagemagick and adobe-source-code-pro font available? \
             Also, check out https://github.com/facebookexperimental/resctl-demo/issues/256",
        )
    }

    pub fn plot_text<'o>(&mut self, out: &mut Box<dyn Write + 'o>) -> Result<()> {
        for (sel, series) in self.res.data.iter() {
            self.plot_one_text(out, sel, series, self.res.mem_profile, &self.res.isol_pct)?;
        }
        Ok(())
    }

    pub fn plot_pdf(&mut self, dir: &Path) -> Result<PathBuf> {
        for (sel, series) in self.res.data.iter() {
            let sr = self.data.sysinfo.sysreqs_report.as_ref().unwrap();
            if let Err(e) = self.plot_one_svg(
                dir,
                sel,
                series,
                self.res.mem_profile,
                &self.res.isol_pct,
                &format!("{}", sr.scr_dev_model.trim()),
            ) {
                bail!("Failed to plot {} graph into {:?} ({})", sel, dir, &e);
            }
        }

        let mut dst = PathBuf::from(dir);
        dst.push("iocost-tune-graphs.pdf");
        let sels = self.res.data.iter().map(|(sel, _)| sel).cloned().collect();
        Self::collect_svgs(dir, sels, &dst)
            .map_err(|e| anyhow!("Failed to collect graphs into {:?} ({})", &dst, &e))?;
        Ok(dst)
    }
}
