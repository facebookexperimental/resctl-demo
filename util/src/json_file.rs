// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use clap;
use log::info;
use serde::{de::DeserializeOwned, Serialize};
use serde_json;
use std::default::Default;
use std::fs;
use std::io::{self, prelude::*};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn read_json<P: AsRef<Path>>(path: P) -> Result<(String, String)> {
    let mut f = fs::OpenOptions::new().read(true).open(path)?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;

    let mut preamble = String::new();
    let mut body = String::new();
    let mut seen_body = false;

    for line in buf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("#") {
            if !seen_body {
                preamble = preamble + line + "\n";
            }
            body = body + "\n";
        } else {
            seen_body = true;
            body = body + line + "\n"
        }
    }
    Ok((preamble, body))
}

pub trait JsonLoad
where
    Self: DeserializeOwned,
{
    fn loaded(&mut self, _prev: Option<&mut Self>) -> Result<()> {
        Ok(())
    }

    fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let (_, body) = read_json(path)?;
        Ok(serde_json::from_str::<Self>(&body)?)
    }
}

pub trait JsonSave
where
    Self: Default + Serialize,
{
    fn preamble() -> Option<String> {
        None
    }

    fn maybe_create_dfl<P: AsRef<Path>>(path_in: P) -> Result<bool> {
        let path = path_in.as_ref();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(&parent)?;
        }

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let data: Self = Default::default();
                f.write_all(data.as_json()?.as_ref())?;
                Ok(true)
            }
            Err(e) => match e.kind() {
                io::ErrorKind::AlreadyExists => Ok(false),
                _ => Err(e.into()),
            },
        }
    }

    fn as_json(&self) -> Result<String> {
        let mut serialized = serde_json::to_string_pretty(&self)?;
        if !serialized.ends_with("\n") {
            serialized += "\n";
        }
        match Self::preamble() {
            Some(pre) => Ok(pre + &serialized),
            None => Ok(serialized),
        }
    }

    fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        f.write_all(self.as_json()?.as_ref())?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct JsonConfigFile<T: JsonLoad + JsonSave> {
    pub path: Option<PathBuf>,
    pub loaded_mod: SystemTime,
    pub data: T,
}

impl<T: JsonLoad + JsonSave + Default> Default for JsonConfigFile<T> {
    fn default() -> Self {
        Self {
            path: None,
            loaded_mod: UNIX_EPOCH,
            data: Default::default(),
        }
    }
}

impl<T: JsonLoad + JsonSave> JsonConfigFile<T> {
    pub fn load<P: AsRef<Path>>(path_in: P) -> Result<Self> {
        let path = AsRef::<Path>::as_ref(&path_in);

        let modified = path.metadata()?.modified()?;
        let mut data = T::load(&path)?;
        data.loaded(None)?;

        Ok(Self {
            path: Some(PathBuf::from(path)),
            loaded_mod: modified,
            data,
        })
    }

    pub fn load_or_create<P: AsRef<Path>>(path_opt: Option<P>) -> Result<Self> {
        match path_opt {
            Some(path_in) => {
                let path = AsRef::<Path>::as_ref(&path_in);

                if T::maybe_create_dfl(&path)? {
                    info!("cfg: Created {:?}", &path);
                }

                Self::load(path)
            }
            None => {
                let mut data: T = Default::default();
                data.loaded(None)?;

                Ok(Self {
                    path: None,
                    loaded_mod: UNIX_EPOCH,
                    data,
                })
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(path) = self.path.as_deref() {
            self.data.save(&path)
        } else {
            Ok(())
        }
    }

    pub fn maybe_reload(&mut self) -> Result<bool> {
        let path = match self.path.as_ref() {
            Some(p) => p,
            None => return Ok(false),
        };

        let modified = fs::metadata(&path)?.modified()?;
        // Consider the file iff it stayed the same for at least 10ms.
        match SystemTime::now().duration_since(modified) {
            Ok(dur) if dur.as_millis() < 10 => return Ok(false),
            _ => {}
        }

        // The same as loaded?
        if self.loaded_mod == modified {
            return Ok(false);
        }

        self.loaded_mod = modified;
        let mut data = T::load(&path)?;
        data.loaded(Some(&mut self.data))?;
        self.data = data;
        Ok(true)
    }
}

pub trait JsonArgs
where
    Self: JsonLoad + JsonSave,
{
    fn match_cmdline() -> clap::ArgMatches<'static>;
    fn verbosity(matches: &clap::ArgMatches) -> u32;
    fn system_configuration_overrides(
        _matches: &clap::ArgMatches,
    ) -> (Option<usize>, Option<usize>, Option<usize>) {
        (None, None, None)
    }
    fn process_cmdline(&mut self, matches: &clap::ArgMatches) -> bool;
}

pub trait JsonArgsHelper
where
    Self: JsonArgs,
{
    fn init_args_and_logging_nosave() -> Result<(JsonConfigFile<Self>, bool)>;
    fn save_args(args_file: &JsonConfigFile<Self>) -> Result<()>;
    fn init_args_and_logging() -> Result<JsonConfigFile<Self>>;
}

impl<T> JsonArgsHelper for T
where
    T: JsonArgs,
{
    fn init_args_and_logging_nosave() -> Result<(JsonConfigFile<T>, bool)> {
        let matches = T::match_cmdline();
        super::init_logging(T::verbosity(&matches));
        let overrides = T::system_configuration_overrides(&matches);
        super::override_system_configuration(overrides.0, overrides.1, overrides.2);

        let mut args_file = JsonConfigFile::<T>::load_or_create(matches.value_of("args").as_ref())?;
        let updated = args_file.data.process_cmdline(&matches);

        Ok((args_file, updated))
    }

    fn save_args(args_file: &JsonConfigFile<T>) -> Result<()> {
        if args_file.path.is_some() {
            info!(
                "Updating command line arguments file {:?}",
                &args_file.path.as_deref().unwrap()
            );
            args_file.save()?;
        }
        Ok(())
    }

    fn init_args_and_logging() -> Result<JsonConfigFile<T>> {
        let (args_file, updated) = Self::init_args_and_logging_nosave()?;
        if updated {
            Self::save_args(&args_file)?;
        }
        Ok(args_file)
    }
}

#[derive(Debug)]
pub struct JsonReportFile<T: JsonSave> {
    pub path: Option<PathBuf>,
    pub staging: PathBuf,
    pub data: T,
}

impl<T: JsonSave> JsonReportFile<T> {
    pub fn new<P: AsRef<Path>>(path_opt: Option<P>) -> Self {
        let (path, staging) = match path_opt {
            Some(p) => {
                let pb = PathBuf::from(p.as_ref());
                let mut st = pb.clone().into_os_string();
                st.push(".staging");
                (Some(pb), PathBuf::from(st))
            }
            None => (None, PathBuf::new()),
        };

        Self {
            path,
            staging,
            data: Default::default(),
        }
    }

    pub fn commit(&self) -> Result<()> {
        let path = match self.path.as_ref() {
            Some(v) => v,
            None => return Ok(()),
        };

        self.data.save(&self.staging)?;
        fs::rename(&self.staging, &path)?;
        Ok(())
    }
}

pub struct JsonRawFile {
    pub path: PathBuf,
    pub preamble: String,
    pub value: serde_json::Value,
}

impl JsonRawFile {
    pub fn load<P: AsRef<Path>>(path_in: P) -> Result<Self> {
        let path = PathBuf::from(path_in.as_ref());
        let (preamble, body) = read_json(&path)?;

        Ok(Self {
            path,
            preamble,
            value: serde_json::from_str(&body)?,
        })
    }

    pub fn save(&self) -> Result<()> {
        let output = self.preamble.clone() + &serde_json::ser::to_string_pretty(&self.value)?;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)?;
        f.write_all(output.as_ref())?;
        Ok(())
    }
}
