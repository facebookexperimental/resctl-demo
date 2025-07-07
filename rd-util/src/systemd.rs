// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::{debug, info, trace, warn};

use zbus::proxy;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{Connection, Proxy};

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{sleep, LocalKey};
use std::time::{Duration, Instant};

pub const SYSTEMD_DFL_TIMEOUT: f64 = 15.0;
const SD1_DST: &str = "org.freedesktop.systemd1";
const SD1_PATH: &str = "/org/freedesktop/systemd1";

std::thread_local!(pub static SYS_SD_BUS: RefCell<SystemdDbus> =
                   RefCell::new(SystemdDbus::new(false).unwrap()));
std::thread_local!(pub static USR_SD_BUS: RefCell<SystemdDbus> =
                   RefCell::new(SystemdDbus::new(true).unwrap()));

lazy_static::lazy_static! {
    static ref SYSTEMD_TIMEOUT_MS: AtomicU64 =
        AtomicU64::new((SYSTEMD_DFL_TIMEOUT * 1000.0).round() as u64);
}

pub fn set_systemd_timeout(timeout: f64) {
    SYSTEMD_TIMEOUT_MS.store((timeout * 1000.0).round() as u64, Ordering::Relaxed);
}

fn systemd_timeout() -> f64 {
    SYSTEMD_TIMEOUT_MS.load(Ordering::Relaxed) as f64 / 1000.0
}

#[derive(Debug, Clone)]
pub enum Prop {
    U32(u32),
    U64(u64),
    Bool(bool),
    String(String),
}

impl<'a> From<Prop> for Value<'a> {
    fn from(prop: Prop) -> Self {
        match prop {
            Prop::U32(v) => Value::U32(v),
            Prop::U64(v) => Value::U64(v),
            Prop::Bool(v) => Value::Bool(v),
            Prop::String(s) => Value::Str(s.into()),
        }
    }
}

fn escape_name(name: &str) -> String {
    let mut escaped = String::new();
    for c in name.chars() {
        let mut buf = [0; 1]; // must be ascii
        let utf8 = c.encode_utf8(&mut buf);

        if c.is_alphanumeric() {
            escaped += utf8;
        } else {
            escaped += &format!("_{:02x}", utf8.bytes().next().unwrap());
        }
    }
    if log::max_level() >= log::LevelFilter::Trace && name != escaped {
        trace!("svc: escaped {:?} -> {:?}", &name, &escaped);
    }
    escaped
}

#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    fn reload(&self) -> Result<()>;
    fn restart_unit(&self, name: &str, mode: &str) -> Result<zbus::zvariant::OwnedObjectPath>;
    fn reset_failed_unit(&self, name: &str) -> Result<()>;
    fn start_unit(&self, name: &str, mode: &str) -> Result<zbus::zvariant::OwnedObjectPath>;
    fn stop_unit(&self, name: &str, mode: &str) -> Result<zbus::zvariant::OwnedObjectPath>;
    fn set_unit_properties(
        &self,
        name: &str,
        runtime: bool,
        properties: Vec<(&str, Value<'_>)>,
    ) -> Result<()>;
    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: Vec<(&str, Value<'_>)>,
        aux: Vec<(&str, Vec<(&str, Value<'_>)>)>,
    ) -> Result<zbus::zvariant::OwnedObjectPath>;
}

pub struct SystemdDbus {
    connection: Connection,
    rt: tokio::runtime::Runtime,
}

impl SystemdDbus {
    fn manager_proxy(&self) -> zbus::Result<SystemdManagerProxy> {
        self.rt.block_on(SystemdManagerProxy::new(&self.connection))
    }

    fn new_int(user: bool) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().unwrap();

        let connection = if user {
            rt.block_on(Connection::session())?
        } else {
            rt.block_on(Connection::system())?
        };

        Ok(SystemdDbus { connection, rt })
    }

    pub fn new(user: bool) -> Result<Self> {
        Self::new_int(user)
    }

    fn daemon_reload(&mut self) -> Result<()> {
        self.rt.block_on(self.manager_proxy().unwrap().reload())
    }

    pub fn get_unit_props<'u>(&mut self, name: &str) -> Result<HashMap<String, Prop>> {
        let path = SD1_PATH.to_string() + "/unit/" + &escape_name(&name);

        let proxy = self.rt.block_on(Proxy::new(
            &self.connection,
            SD1_DST,
            path,
            "org.freedesktop.DBus.Properties",
        ))?;

        let props_owned: HashMap<String, OwnedValue> =
            self.rt.block_on(proxy.call("GetAll", &("",)))?;

        let mut props = HashMap::new();

        /* prepare for usage later */
        for (key, owned_value) in &props_owned {
            match &**owned_value {
                Value::Str(v) => {
                    props.insert(key.into(), Prop::String(v.to_string()));
                }
                Value::U64(v) => {
                    props.insert(key.into(), Prop::U64(*v));
                }
                Value::U32(v) => {
                    props.insert(key.into(), Prop::U32(*v));
                }
                Value::Bool(v) => {
                    props.insert(key.into(), Prop::Bool(*v));
                }
                _ => {}
            }
        }

        Ok(props)
    }

    fn set_unit_props(&mut self, name: &str, props: Vec<(String, Prop)>) -> Result<()> {
        let map_props: Vec<(&str, Value)> = props
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone().into()))
            .collect();

        self.rt.block_on(
            self.manager_proxy()
                .unwrap()
                .set_unit_properties(name, false, map_props),
        )?;
        Ok(())
    }

    pub fn start_unit(&mut self, name: &str) -> Result<()> {
        self.rt
            .block_on(self.manager_proxy().unwrap().start_unit(name, "fail"))?;
        Ok(())
    }

    pub fn stop_unit(&mut self, name: &str) -> Result<()> {
        self.rt
            .block_on(self.manager_proxy().unwrap().stop_unit(name, "fail"))?;
        Ok(())
    }

    pub fn reset_failed_unit(&mut self, name: &str) -> Result<()> {
        self.rt
            .block_on(self.manager_proxy().unwrap().reset_failed_unit(name))?;
        Ok(())
    }

    pub fn restart_unit(&mut self, name: &str) -> Result<()> {
        self.rt
            .block_on(self.manager_proxy().unwrap().restart_unit(name, "fail"))?;
        Ok(())
    }

    pub fn start_transient_svc(
        &mut self,
        name: String,
        args: Vec<String>,
        envs: Vec<String>,
        extra_props: Vec<(String, Prop)>,
    ) -> Result<()> {
        // NAME(s) JOB_MODE(s) PROPS(a(sv)) AUX_UNITS(a(s a(sv)))
        //
        // PROPS:
        // ["Description"] = str,
        // ["Slice"] = str,
        // ["CPUWeight"] = num,
        // ...
        // ["Environment"] = ([ENV0]=str, [ENV1]=str...)
        // ["ExecStart"] = (args[0], (args[0], args[1], ...), false)

        // desc string
        let desc = args.iter().fold(name.clone(), |mut a, i| {
            a += " ";
            a += i;
            a
        });

        let exec_start = Value::from(vec![(args[0].clone(), args, false)]);

        let mut props: Vec<(&str, Value)> = vec![
            ("Description", Value::from(desc)),
            ("Environment", Value::from(envs)),
            ("ExecStart", exec_start),
        ];

        let map_extra_props: Vec<(&str, Value)> = extra_props
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone().into()))
            .collect();

        props.extend(map_extra_props);

        let job = self
            .rt
            .block_on(self.manager_proxy().unwrap().start_transient_unit(
                name.as_str(),
                "fail",
                props,
                vec![],
            ))?;
        debug!("Started transient unit: {job}");

        Ok(())
    }
}

pub fn daemon_reload() -> Result<()> {
    SYS_SD_BUS.with(|s| s.borrow_mut().daemon_reload())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitState {
    NotFound,
    Running,
    Exited,
    OtherActive(String),
    Inactive(String),
    Failed(String),
    Other(String),
}

use UnitState as US;

impl Default for UnitState {
    fn default() -> Self {
        US::NotFound
    }
}

#[derive(Debug)]
pub struct UnitProps {
    props: HashMap<String, Prop>,
}

impl UnitProps {
    fn new(init_props: &HashMap<String, Prop>) -> Result<Self> {
        Ok(Self {
            props: init_props.clone(),
        })
    }

    pub fn string<'a>(&'a self, key: &str) -> Option<&'a str> {
        match self.props.get(key) {
            Some(Prop::String(v)) => Some(v),
            _ => None,
        }
    }

    pub fn u64_dfl_max(&self, key: &str) -> Option<u64> {
        match self.props.get(key) {
            Some(Prop::U64(v)) if *v < std::u64::MAX => Some(*v),
            _ => None,
        }
    }

    pub fn u64_dfl_zero(&self, key: &str) -> Option<u64> {
        match self.props.get(key) {
            Some(Prop::U64(v)) if *v > 0 => Some(*v),
            _ => None,
        }
    }

    fn state(&self) -> US {
        let v = self.string("LoadState");
        match v {
            Some("loaded") => {}
            Some("not-found") => return US::NotFound,
            Some(_) => return US::Other(v.unwrap().into()),
            None => return US::Other("no-load-state".into()),
        };

        let ss = match self.string("SubState") {
            Some(v) => v.to_string(),
            None => "no-sub-state".to_string(),
        };

        match self.string("ActiveState") {
            Some("active") => match ss.as_str() {
                "running" => US::Running,
                "exited" => US::Exited,
                _ => US::OtherActive(ss),
            },
            Some("inactive") => US::Inactive(ss.into()),
            Some("failed") => US::Failed(ss.into()),
            Some(v) => US::Other(format!("{}:{}", v, ss)),
            None => US::Other("no-active-state".into()),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UnitResCtl {
    pub cpu_weight: Option<u64>,
    pub io_weight: Option<u64>,
    pub mem_min: Option<u64>,
    pub mem_low: Option<u64>,
    pub mem_high: Option<u64>,
    pub mem_max: Option<u64>,
}

impl fmt::Display for UnitResCtl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "cpu_w={:?} io_w={:?} mem={:?}:{:?}:{:?}:{:?}",
            &self.cpu_weight,
            &self.io_weight,
            &self.mem_min,
            &self.mem_low,
            &self.mem_high,
            &self.mem_max
        )
    }
}

#[derive(Debug)]
pub struct Unit {
    pub user: bool,
    pub name: String,
    pub state: US,
    pub resctl: UnitResCtl,
    pub props: UnitProps,
    pub quiet: bool,
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let user_str = match self.user {
            true => "(user)",
            false => "",
        };
        write!(
            f,
            "{}{}: state={:?} {}",
            &self.name, &user_str, &self.state, &self.resctl,
        )
    }
}

impl Unit {
    pub fn new(user: bool, name: String) -> Result<Self> {
        let sb = match user {
            false => &SYS_SD_BUS,
            true => &USR_SD_BUS,
        };
        let mut svc = Self {
            user,
            state: US::Other("unknown".into()),
            resctl: Default::default(),
            props: UnitProps::new(&(sb.with(|s| s.borrow_mut().get_unit_props(&name))?))?,
            quiet: false,
            name,
        };
        svc.refresh_fields();
        Ok(svc)
    }

    pub fn new_sys(name: String) -> Result<Self> {
        Self::new(false, name)
    }

    pub fn new_user(name: String) -> Result<Self> {
        Self::new(true, name)
    }

    pub fn sd_bus(&self) -> &'static LocalKey<RefCell<SystemdDbus>> {
        match self.user {
            false => &SYS_SD_BUS,
            true => &USR_SD_BUS,
        }
    }

    pub fn refresh(&mut self) -> Result<()> {
        trace!("svc: {:?} refreshing", &self.name);
        self.props = match self
            .sd_bus()
            .with(|s| s.borrow_mut().get_unit_props(&self.name))
        {
            Ok(props) => UnitProps::new(&props)?,
            Err(e) => {
                debug!(
                    "Failed to unmarshall response from {}, assuming gone ({:?})",
                    &self.name, &e
                );
                self.state = US::NotFound;
                return Err(e);
            }
        };
        self.refresh_fields();
        Ok(())
    }

    pub fn refresh_fields(&mut self) {
        let new_state = self.props.state();

        if !self.quiet && self.state == US::Running {
            match &new_state {
                US::Running => {}
                US::Exited => info!("svc: {:?} exited", &self.name),
                US::Failed(how) => info!("svc: {:?} failed ({:?})", &self.name, &how),
                US::NotFound => info!("svc: {:?} is gone", &self.name),
                s => info!(
                    "svc: {:?} transitioned from Running to {:?}",
                    &self.name, &s
                ),
            }
        }

        self.state = new_state;
        self.resctl.cpu_weight = self.props.u64_dfl_max("CPUWeight");
        self.resctl.io_weight = self.props.u64_dfl_max("IOWeight");
        self.resctl.mem_min = self.props.u64_dfl_zero("MemoryMin");
        self.resctl.mem_low = self.props.u64_dfl_zero("MemoryLow");
        self.resctl.mem_high = self.props.u64_dfl_max("MemoryHigh");
        self.resctl.mem_max = self.props.u64_dfl_max("MemoryMax");
    }

    pub fn resctl_props(&self) -> Vec<(String, Prop)> {
        vec![
            (
                "CPUWeight".into(),
                Prop::U64(self.resctl.cpu_weight.unwrap_or(u64::MAX)),
            ),
            (
                "IOWeight".into(),
                Prop::U64(self.resctl.io_weight.unwrap_or(u64::MAX)),
            ),
            (
                "MemoryMin".into(),
                Prop::U64(self.resctl.mem_min.unwrap_or(0)),
            ),
            (
                "MemoryLow".into(),
                Prop::U64(self.resctl.mem_low.unwrap_or(0)),
            ),
            (
                "MemoryHigh".into(),
                Prop::U64(self.resctl.mem_high.unwrap_or(std::u64::MAX)),
            ),
            (
                "MemoryMax".into(),
                Prop::U64(self.resctl.mem_max.unwrap_or(std::u64::MAX)),
            ),
        ]
    }

    pub fn apply(&mut self) -> Result<()> {
        trace!("svc: {:?} applying resctl", &self.name);
        self.sd_bus().with(|s| {
            s.borrow_mut()
                .set_unit_props(&self.name, self.resctl_props())
        })?;
        self.refresh()
    }

    pub fn set_prop(&mut self, key: &str, prop: Prop) -> Result<()> {
        self.sd_bus().with(|s| {
            s.borrow_mut()
                .set_unit_props(&self.name, vec![(key.into(), prop)])
        })?;
        self.refresh()
    }

    fn wait_transition<F>(&mut self, wait_till: F, timeout: f64, exiting_timeout: f64)
    where
        F: Fn(&US) -> bool,
    {
        let started_at = Instant::now();
        loop {
            if let Ok(()) = self.refresh() {
                trace!(
                    "svc: {:?} waiting transitions ({:?})",
                    &self.name,
                    &self.state
                );
                match &self.state {
                    US::OtherActive(_) | US::Other(_) => {}
                    state if !wait_till(state) => {}
                    _ => return,
                }
            }

            let dur = Duration::from_secs_f64(match super::prog_exiting() {
                false => timeout,
                true => exiting_timeout,
            });
            if Instant::now().duration_since(started_at) >= dur {
                trace!("svc: {:?} waiting transitions timed out", &self.name);
                return;
            }

            sleep(Duration::from_millis(100));
        }
    }

    pub fn stop(&mut self) -> Result<bool> {
        debug!("svc: {:?} stopping ({:?})", &self.name, &self.state);

        self.refresh()?;
        match self.state {
            US::NotFound | US::Failed(_) => {
                debug!("svc: {:?} already stopped ({:?})", &self.name, &self.state);
                return Ok(true);
            }
            _ => {}
        }

        self.sd_bus()
            .with(|s| s.borrow_mut().stop_unit(&self.name))?;
        // We're used from exit paths. Force a bit of wait so that we
        // can shut down gracefully in most cases.
        self.wait_transition(
            |x| *x != US::Running,
            systemd_timeout(),
            systemd_timeout() / 5.0,
        );
        if !self.quiet {
            info!("svc: {:?} stopped ({:?})", &self.name, &self.state);
        }
        match self.state {
            US::NotFound | US::Failed(_) => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn stop_and_reset(&mut self) -> Result<()> {
        self.stop()?;
        if let US::Failed(_) = self.state {
            self.sd_bus()
                .with(|s| s.borrow_mut().reset_failed_unit(&self.name))?;
            // We're used from exit paths. Force a bit of wait so that we
            // can shut down gracefully in most cases.
            self.wait_transition(
                |x| *x == US::NotFound,
                systemd_timeout(),
                systemd_timeout() / 5.0,
            );
        }
        match self.state {
            US::NotFound => Ok(()),
            _ => bail!(
                "invalid post-reset state {:?} for {}",
                self.state,
                &self.name
            ),
        }
    }

    pub fn try_start_nowait(&mut self) -> Result<()> {
        debug!("svc: {:?} starting ({:?})", &self.name, &self.state);
        self.sd_bus()
            .with(|s| s.borrow_mut().start_unit(&self.name))
    }

    pub fn try_start(&mut self) -> Result<bool> {
        self.try_start_nowait()?;
        self.wait_transition(
            |x| match x {
                US::Running | US::Exited | US::Failed(_) => true,
                _ => false,
            },
            systemd_timeout(),
            0.0,
        );
        if !self.quiet {
            info!("svc: {:?} started ({:?})", &self.name, &self.state);
        }
        match self.state {
            US::Running | US::Exited => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn restart(&mut self) -> Result<()> {
        if !self.quiet {
            info!("svc: {:?} restarting ({:?})", &self.name, &self.state);
        }
        self.sd_bus()
            .with(|s| s.borrow_mut().restart_unit(&self.name))
    }
}

pub struct TransientService {
    pub unit: Unit,
    pub args: Vec<String>,
    pub envs: Vec<String>,
    pub extra_props: HashMap<String, Prop>,
    pub keep: bool,
}

impl TransientService {
    pub fn new(
        user: bool,
        name: String,
        args: Vec<String>,
        envs: Vec<String>,
        umask: Option<u32>,
    ) -> Result<Self> {
        if !name.ends_with(".service") {
            bail!("invalid service name {}", &name);
        }
        let mut svc = Self {
            unit: Unit::new(user, name)?,
            args,
            envs,
            extra_props: HashMap::new(),
            keep: false,
        };
        svc.add_prop("RemainAfterExit".into(), Prop::Bool(true));
        if let Some(v) = umask {
            svc.add_prop("UMask".into(), Prop::U32(v));
        }
        Ok(svc)
    }

    pub fn new_sys(
        name: String,
        args: Vec<String>,
        envs: Vec<String>,
        umask: Option<u32>,
    ) -> Result<Self> {
        Self::new(false, name, args, envs, umask)
    }

    pub fn new_user(
        name: String,
        args: Vec<String>,
        envs: Vec<String>,
        umask: Option<u32>,
    ) -> Result<Self> {
        Self::new(true, name, args, envs, umask)
    }

    pub fn add_prop(&mut self, key: String, v: Prop) -> &mut Self {
        self.extra_props.insert(key, v);
        self
    }

    pub fn del_prop(&mut self, key: &String) -> (&mut Self, Option<Prop>) {
        let v = self.extra_props.remove(key);
        (self, v)
    }

    pub fn set_slice(&mut self, slice: &str) -> &mut Self {
        self.add_prop("Slice".into(), Prop::String(slice.into()));
        self
    }

    pub fn set_working_dir(&mut self, dir: &str) -> &mut Self {
        self.add_prop("WorkingDirectory".into(), Prop::String(dir.into()));
        self
    }

    pub fn set_restart_always(&mut self) -> &mut Self {
        self.add_prop("Restart".into(), Prop::String("always".into()));
        self
    }

    pub fn set_quiet(&mut self) -> &mut Self {
        self.unit.quiet = true;
        self
    }

    fn try_start(&mut self) -> Result<bool> {
        let mut extra_props = self.unit.resctl_props();
        extra_props.extend(self.extra_props.clone());

        debug!(
            "svc: {:?} starting ({:?})",
            &self.unit.name, &self.unit.state
        );
        self.unit.sd_bus().with(|s| {
            s.borrow_mut().start_transient_svc(
                self.unit.name.clone(),
                self.args.clone(),
                self.envs.clone(),
                extra_props,
            )
        })?;

        self.unit.wait_transition(
            |x| match x {
                US::Running | US::Exited | US::Failed(_) => true,
                _ => false,
            },
            systemd_timeout(),
            0.0,
        );
        if !self.unit.quiet {
            info!(
                "svc: {:?} started ({:?})",
                &self.unit.name, &self.unit.state
            );
        }
        match self.unit.state {
            US::Running | US::Exited => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        let resctl = self.unit.resctl.clone();
        match self.unit.stop_and_reset() {
            Ok(()) => {
                self.unit.resctl = resctl;
                match self.try_start() {
                    Ok(true) => Ok(()),
                    Ok(false) => bail!("invalid service state {:?}", &self.unit.state),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }
}

impl Drop for TransientService {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for tries in (1..6).rev() {
            let action = match tries {
                0 => String::new(),
                v => format!(", retrying... ({} tries left)", v),
            };
            match self.unit.stop_and_reset() {
                Ok(()) => {}
                Err(e) => warn!(
                    "Failed to stop {} on drop ({:?}){}",
                    &self.unit.name, &e, action
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TransientService, UnitState};
    use log::{info, trace};
    use std::thread::sleep;
    use std::time::Duration;

    //#[test]
    // TODO: This test is not hermetic as it has an implicit dependency
    // on the systemd session bus; it should be spinning up its own bus instead.
    #[allow(dead_code)]
    fn test_transient_service() {
        let _ = ::env_logger::try_init();
        let name = "test-transient.service";

        info!("Creating {}", &name);
        let mut svc = TransientService::new_user(
            name.into(),
            vec![
                "/usr/bin/bash".into(),
                "-c".into(),
                "echo $TEST_ENV; sleep 3".into(),
            ],
            vec![("TEST_ENV=TEST".into())],
            None,
        )
        .unwrap();
        assert_eq!(svc.unit.state, UnitState::NotFound);

        info!("Starting the service");
        svc.start().unwrap();

        trace!("{} props={:#?}", &name, &svc.unit.props);
        info!("{}", &svc.unit);

        info!("Setting cpu weight to 111");
        let cpu_weight = svc.unit.resctl.cpu_weight;
        svc.unit.resctl.cpu_weight = Some(111);
        svc.unit.apply().unwrap();
        info!("{}", &svc.unit);
        assert_eq!(svc.unit.resctl.cpu_weight, Some(111));

        info!("Restoring cpu weight");
        svc.unit.resctl.cpu_weight = cpu_weight;
        svc.unit.apply().unwrap();
        info!("{}", &svc.unit);
        assert_eq!(svc.unit.resctl.cpu_weight, cpu_weight);

        info!("Sleeping 4 secs and checking state, it should have exited");
        sleep(Duration::from_secs(4));
        svc.unit.refresh().unwrap();
        info!("{}", &svc.unit);
        assert_eq!(svc.unit.state, UnitState::Exited);

        info!("Restarting the service w/o RemainAfterExit");
        svc.del_prop(&"RemainAfterExit".to_string());
        svc.start().unwrap();

        info!("Sleeping 4 secs and checking state, it should be gone");
        sleep(Duration::from_secs(4));
        svc.unit.refresh().unwrap();
        info!("{}", &svc.unit);
        assert_eq!(svc.unit.state, UnitState::NotFound);

        info!("Dropping the service");
        drop(svc);
        info!("Dropped");
    }
}
