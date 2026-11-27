//! The host file system as a [`WorldSource`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use wgc_core::source::{Config, Entry, WorldSource};
use yaml_rust2::{Yaml, YamlLoader};

use crate::layout::LayoutError;
use crate::plan::PlanError;

/// Reads world files from the host file system. Paths are used as they are.
#[derive(Debug, Clone, Copy, Default)]
pub struct FsSource;

fn read_error(path: &Path) -> impl FnOnce(io::Error) -> PlanError + '_ {
    move |source| PlanError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl WorldSource for FsSource {
    type Path = PathBuf;
    type Error = PlanError;

    fn read(&self, path: &PathBuf) -> Result<Vec<u8>, PlanError> {
        fs::read(path).map_err(read_error(path))
    }

    fn read_optional(&self, path: &PathBuf) -> Result<Option<Vec<u8>>, PlanError> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(read_error(path)(e)),
        }
    }

    fn size(&self, path: &PathBuf) -> Result<u64, PlanError> {
        Ok(fs::metadata(path).map_err(read_error(path))?.len())
    }

    fn exists(&self, path: &PathBuf) -> bool {
        path.exists()
    }

    fn list(&self, dir: &PathBuf) -> Result<Vec<Entry<PathBuf>>, PlanError> {
        let access = |e| PlanError::Layout(LayoutError::io(dir, e));
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(access(e)),
        };
        entries
            .map(|entry| {
                let path = entry.map_err(access)?.path();
                Ok(Entry {
                    is_file: path.is_file(),
                    path,
                })
            })
            .collect()
    }

    fn join(&self, base: &PathBuf, relative: &str) -> PathBuf {
        base.join(relative)
    }

    fn name<'a>(&self, path: &'a PathBuf) -> Option<&'a str> {
        path.file_name()?.to_str()
    }

    fn read_config(&self, path: &PathBuf) -> Result<Option<Config>, PlanError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(read_error(path)(e)),
        };
        let docs = YamlLoader::load_from_str(&text).map_err(|e| {
            read_error(path)(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })?;
        Ok(Some(docs.into_iter().next().map_or(Config::Null, config)))
    }
}

fn config(yaml: Yaml) -> Config {
    match yaml {
        Yaml::Null => Config::Null,
        Yaml::Integer(v) => Config::Integer(v),
        Yaml::String(s) => Config::String(s),
        Yaml::Hash(map) => Config::Map(
            map.into_iter()
                .filter_map(|(key, value)| match key {
                    Yaml::String(key) => Some((key, config(value))),
                    _ => None,
                })
                .collect(),
        ),
        _ => Config::Other,
    }
}
