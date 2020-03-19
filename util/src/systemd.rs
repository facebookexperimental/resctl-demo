// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use dbus;
use dbus::arg::{RefArg, Variant};
use dbus::blocking::{BlockingSender, Connection};
use dbus::Message;
use lazy_static::lazy_static;
use log::{debug, info, trace, warn};
use std::collections::HashMap;
use std::fmt;
use std::thread::{sleep, LocalKey};
use std::thread_local;
use std::time::{Duration, Instant};
use std::u64;

type PropMap = HashMap<String, Variant<Box<dyn RefArg>>>;
type PropVec = Vec<(String, Variant<Box<dyn RefArg>>)>;

const SD1_DST: &str = "org.freedesktop.systemd1";
const SD1_PATH: &str = "/org/freedesktop/systemd1";
const DBUS_TIMEOUT_MS: u64 = 15000;

lazy_static! {
    static ref DBUS_TIMEOUT: Duration = Duration::from_millis(DBUS_TIMEOUT_MS);
}
thread_local!(pub static SYS_SD_BUS: SystemdDbus = SystemdDbus::new(false).unwrap());
thread_local!(pub static USR_SD_BUS: SystemdDbus = SystemdDbus::new(true).unwrap());

pub enum Prop {
    U32(u32),
    U64(u64),
    Bool(bool),
    String(String),
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

fn new_unit_msg(name: &str, intf: &str, method: &str) -> Result<Message> {
    let path = SD1_PATH.to_string() + "/unit/" + &escape_name(&name);
    match Message::new_method_call(SD1_DST, &path, intf, method) {
        Ok(v) => Ok(v),
        Err(e) => bail!("{}", &e),
    }
}

fn new_sd1_msg(method: &str) -> Result<Message> {
    match Message::new_method_call(
        SD1_DST,
        SD1_PATH,
        "org.freedesktop.systemd1.Manager",
        method,
    ) {
        Ok(v) => Ok(v),
        Err(e) => bail!("{}", &e),
    }
}

fn new_start_transient_svc_msg(
    name: String,
    args: Vec<String>,
    envs: Vec<String>,
    extra_props: PropVec,
) -> Result<Message> {
    // NAME(s) JOB_MODE(s) PROPS(a(sv)) AUX_UNITS(a(s a(sv)))
    //
    // PROPS:
    // ["Description"] = str,
    // ["Slice"] = str,
    // ["CPUWeight"] = num,
    // ...
    // ["Environment"] = ([ENV0]=str, [ENV1]=str...)
    // ["ExecStart"] = (args[0], (args[0], args[1], ...), false)
    let m = new_sd1_msg("StartTransientUnit")?;

    // name and job_mode
    let m = m.append2(name.clone(), "fail");

    // props
    let desc = args.iter().fold(name, |mut a, i| {
        a += " ";
        a += i;
        a
    });

    // props["ExecStart"]
    let args_cont: Vec<(String, Vec<String>, bool)> = vec![(args[0].clone(), args, false)];

    // props["Environment"]
    let mut props: PropVec = vec![
        ("Description".into(), Variant(Box::new(desc))),
        ("Environment".into(), Variant(Box::new(envs))),
        ("ExecStart".into(), Variant(Box::new(args_cont))),
    ];
    props.extend(extra_props);
    let m = m.append1(props);

    // No aux units
    let aux: Vec<(String, PropVec)> = Vec::new();
    Ok(m.append1(aux))
}

pub struct SystemdDbus {
    pub conn: Connection,
}

impl SystemdDbus {
    pub fn new(user: bool) -> Result<Self> {
        let conn = match user {
            false => Connection::new_system()?,
            true => Connection::new_session()?,
        };
        Ok(Self { conn })
    }

    pub fn daemon_reload(&self) -> Result<()> {
        let m = new_sd1_msg("Reload")?;
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn get_unit_props<'u>(&self, name: &str) -> Result<UnitProps> {
        let m = new_unit_msg(&name, "org.freedesktop.DBus.Properties", "GetAll")?.append1("");
        let r = self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(UnitProps {
            name: name.into(),
            props: r.read1()?,
        })
    }

    pub fn set_unit_props(&self, name: &str, props: PropVec) -> Result<()> {
        let m = new_sd1_msg("SetUnitProperties")?.append3(name, true, props);
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn start_unit(&self, name: &str) -> Result<()> {
        let m = new_sd1_msg("StartUnit")?.append2(&name, "fail");
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn stop_unit(&self, name: &str) -> Result<()> {
        let m = new_sd1_msg("StopUnit")?.append2(&name, "fail");
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn reset_failed_unit(&self, name: &str) -> Result<()> {
        let m = new_sd1_msg("ResetFailedUnit")?.append1(&name);
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn restart_unit(&self, name: &str) -> Result<()> {
        let m = new_sd1_msg("RestartUnit")?.append2(&name, "fail");
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }

    pub fn start_transient_svc(
        &self,
        name: String,
        args: Vec<String>,
        envs: Vec<String>,
        extra_props: PropVec,
    ) -> Result<()> {
        let m = new_start_transient_svc_msg(name, args, envs, extra_props)?;
        self.conn.send_with_reply_and_block(m, *DBUS_TIMEOUT)?;
        Ok(())
    }
}

pub fn daemon_reload() -> Result<()> {
    SYS_SD_BUS.with(|s| s.daemon_reload())
}

#[derive(Debug, Default)]
pub struct UnitProps {
    name: String,
    props: PropMap,
}

// Force Send&Sync for the props cache. This should be safe.
unsafe impl Send for UnitProps {}
unsafe impl Sync for UnitProps {}

#[derive(Debug, PartialEq, Eq)]
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

impl UnitProps {
    pub fn string(&self, key: &str) -> Option<String> {
        self.props
            .get(key)
            .and_then(|x| x.as_str())
            .and_then(|x| Some(x.to_string()))
    }

    pub fn u64_dfl_max(&self, key: &str) -> Option<u64> {
        match self.props.get(key) {
            Some(v) => match v.as_u64() {
                Some(v) if v < u64::MAX => Some(v),
                _ => None,
            },
            None => None,
        }
    }

    pub fn u64_dfl_zero(&self, key: &str) -> Option<u64> {
        match self.props.get(key) {
            Some(v) => match v.as_u64() {
                Some(v) if v > 0 => Some(v),
                _ => None,
            },
            None => None,
        }
    }

    fn state(&self) -> US {
        let v = self.string("LoadState");
        match v.as_deref() {
            Some("loaded") => (),
            Some("not-found") => return US::NotFound,
            Some(_) => return US::Other(v.unwrap()),
            None => return US::Other("no-load-state".into()),
        };

        let ss = match self.string("SubState") {
            Some(v) => v,
            None => "no-sub-state".to_string(),
        };

        match self.string("ActiveState").as_deref() {
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
            props: sb.with(|s| s.get_unit_props(&name))?,
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

    pub fn sd_bus(&self) -> &'static LocalKey<SystemdDbus> {
        match self.user {
            false => &SYS_SD_BUS,
            true => &USR_SD_BUS,
        }
    }

    pub fn refresh(&mut self) -> Result<()> {
        trace!("svc: {:?} refreshing", &self.name);
        self.props = match self.sd_bus().with(|s| s.get_unit_props(&self.name)) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to refresh {} ({:?})", &self.name, &e);
                self.state = US::NotFound;
                return Err(e);
            }
        };
        self.refresh_fields();
        Ok(())
    }

    pub fn refresh_fields(&mut self) {
        let new_state = self.props.state();

        if self.state == US::Running {
            match &new_state {
                US::Running => (),
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

    pub fn apply(&mut self) -> Result<()> {
        trace!("svc: {:?} applying resctl", &self.name);
        let props: PropVec = vec![
            (
                "CPUWeight".into(),
                Variant(Box::new(self.resctl.cpu_weight.unwrap_or(u64::MAX))),
            ),
            (
                "IOWeight".into(),
                Variant(Box::new(self.resctl.io_weight.unwrap_or(u64::MAX))),
            ),
            (
                "MemoryMin".into(),
                Variant(Box::new(self.resctl.mem_min.unwrap_or(0))),
            ),
            (
                "MemoryLow".into(),
                Variant(Box::new(self.resctl.mem_low.unwrap_or(0))),
            ),
            (
                "MemoryHigh".into(),
                Variant(Box::new(self.resctl.mem_high.unwrap_or(u64::MAX))),
            ),
            (
                "MemoryMax".into(),
                Variant(Box::new(self.resctl.mem_max.unwrap_or(u64::MAX))),
            ),
        ];
        self.sd_bus()
            .with(|s| s.set_unit_props(&self.name, props))?;
        self.refresh()
    }

    pub fn set_prop(&mut self, key: &str, prop: Prop) -> Result<()> {
        let props: PropVec = vec![(
            key.to_string(),
            match prop {
                Prop::U32(v) => Variant(Box::new(v)),
                Prop::U64(v) => Variant(Box::new(v)),
                Prop::Bool(v) => Variant(Box::new(v)),
                Prop::String(v) => Variant(Box::new(v)),
            },
        )];
        self.sd_bus()
            .with(|s| s.set_unit_props(&self.name, props))?;
        self.refresh()
    }

    fn wait_transition<F>(&mut self, wait_till: F, timeout: Duration)
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
                    US::OtherActive(_) | US::Other(_) => (),
                    state if !wait_till(state) => (),
                    _ => return,
                }
            }

            if Instant::now().duration_since(started_at) >= timeout {
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
            _ => (),
        }

        self.sd_bus().with(|s| s.stop_unit(&self.name))?;
        self.wait_transition(|x| *x != US::Running, *DBUS_TIMEOUT);
        info!("svc: {:?} stopped ({:?})", &self.name, &self.state);
        match self.state {
            US::NotFound | US::Failed(_) => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn stop_and_reset(&mut self) -> Result<()> {
        self.stop()?;
        if let US::Failed(_) = self.state {
            self.sd_bus().with(|s| s.reset_failed_unit(&self.name))?;
            self.wait_transition(|x| *x == US::NotFound, *DBUS_TIMEOUT);
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

    pub fn try_start(&mut self) -> Result<bool> {
        debug!("svc: {:?} starting ({:?})", &self.name, &self.state);
        self.sd_bus().with(|s| s.start_unit(&self.name))?;
        self.wait_transition(
            |x| match x {
                US::Running | US::Failed(_) => true,
                _ => false,
            },
            *DBUS_TIMEOUT,
        );
        info!("svc: {:?} started ({:?})", &self.name, &self.state);
        match self.state {
            US::Running => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn restart(&mut self) -> Result<()> {
        info!("svc: {:?} restarting ({:?})", &self.name, &self.state);
        self.sd_bus().with(|s| s.restart_unit(&self.name))
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
            args: args,
            envs: envs,
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

    fn try_start(&mut self) -> Result<bool> {
        let mut pv: PropVec = Vec::new();
        for (k, v) in self.extra_props.iter() {
            match v {
                Prop::U32(v) => pv.push((k.clone(), Variant(Box::new(*v)))),
                Prop::U64(v) => pv.push((k.clone(), Variant(Box::new(*v)))),
                Prop::Bool(v) => pv.push((k.clone(), Variant(Box::new(*v)))),
                Prop::String(v) => pv.push((k.clone(), Variant(Box::new(v.clone())))),
            }
        }

        debug!(
            "svc: {:?} starting ({:?})",
            &self.unit.name, &self.unit.state
        );
        self.unit.sd_bus().with(|s| {
            s.start_transient_svc(
                self.unit.name.clone(),
                self.args.clone(),
                self.envs.clone(),
                pv,
            )
        })?;

        self.unit.wait_transition(
            |x| match x {
                US::Running | US::Failed(_) => true,
                _ => false,
            },
            *DBUS_TIMEOUT,
        );
        info!(
            "svc: {:?} started ({:?})",
            &self.unit.name, &self.unit.state
        );
        match self.unit.state {
            US::Running => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        match self.unit.stop_and_reset() {
            Ok(()) => match self.try_start() {
                Ok(true) => Ok(()),
                Ok(false) => bail!("invalid service state {:?}", &self.unit.state),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        }
    }
}

impl Drop for TransientService {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        match self.unit.stop_and_reset() {
            Ok(()) => (),
            Err(e) => warn!("Failed to stop {} on drop ({:?})", &self.unit.name, &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TransientService, UnitState};
    use log::{info, trace};
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
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
