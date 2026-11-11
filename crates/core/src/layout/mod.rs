//! Recognizing the world layout.
//!
//! Layout detection follows TMCJapan/sekai (`crates/world/src/tree/hostworld/`)
//! so that both tools classify a folder the same way.

mod data;
mod files;
mod walk;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::nbt::{NbtError, RuleValue};
use crate::region::parse_region_name;
use crate::types::{DataVersion, DimensionId, FileKind};

pub use data::{
    forced_chunks, paper_defaults_config, paper_world_config, player_data_dir, spawn_mobs,
};
pub use files::{DimensionFiles, TripleFiles, list_dimension_files};
pub use walk::{WorldFile, WorldFiles, files_in, world_files};

/// The four layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayoutKind {
    /// `region/`, `DIM-1/`, `DIM1/` in the world folder.
    VanillaLegacy,
    /// 26.1 and later: `dimensions/<ns>/<name>/`.
    Vanilla,
    /// Bukkit before 26.1: `<base>/`, `<base>_nether/DIM-1/`, `<base>_the_end/DIM1/`.
    BukkitLegacy,
    /// Bukkit 26.1 and later: `<base>/dimensions/<ns>/<name>/`.
    Bukkit,
}

impl LayoutKind {
    pub const fn is_bukkit(self) -> bool {
        matches!(self, LayoutKind::Bukkit | LayoutKind::BukkitLegacy)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            LayoutKind::VanillaLegacy => "vanilla_legacy",
            LayoutKind::Vanilla => "vanilla",
            LayoutKind::BukkitLegacy => "bukkit_legacy",
            LayoutKind::Bukkit => "bukkit",
        }
    }
}

/// One dimension of the world.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dimension {
    pub id: DimensionId,
    /// The folder that holds `region/`, `entities/` and `poi/`.
    pub dir: PathBuf,
    /// The world folder this dimension belongs to (where `session.lock` is).
    pub world_folder: PathBuf,
}

/// The result of layout detection. Every later step finds the world's files through this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldLayout {
    /// The input path, canonicalized.
    pub root: PathBuf,
    pub kind: LayoutKind,
    /// Only for Bukkit layouts.
    pub base: Option<String>,
    /// Where `level.dat` and player data are.
    pub main_world_folder: PathBuf,
    /// Units of locking and output, sorted.
    pub world_folders: Vec<PathBuf>,
    /// Only for Bukkit layouts: the root.
    pub server_root: Option<PathBuf>,
    /// `Data.DataVersion` of the main `level.dat`.
    pub data_version: Option<DataVersion>,
    /// The mob spawning rule stored in the main `level.dat` (before 4772).
    pub level_spawn_mobs: RuleValue,
    /// Sorted by ID.
    pub dimensions: Vec<Dimension>,
}

/// Why the layout cannot be recognized.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("no world found at {path}")]
    NotAWorld { path: PathBuf },
    /// W-1: the path looks like the root of a vanilla server rather than a
    /// world folder; `suggested` is the world folder inside it.
    #[error("looks like a vanilla server root; world folder is {suggested}")]
    LooksLikeVanillaServer { suggested: PathBuf },
    /// W-2: the path looks like one world folder of a Bukkit-based server
    /// rather than the server root.
    #[error("looks like a world folder of a bukkit-based server")]
    LooksLikeBukkitWorldFolder,
    #[error("{first} and {second} name the same region")]
    AmbiguousRegion { first: PathBuf, second: PathBuf },
    #[error("{first} and {second} are the same dimension")]
    DuplicateDimension { first: PathBuf, second: PathBuf },
    #[error("symbolic link loop at {path}")]
    SymlinkLoop { path: PathBuf },
    #[error("level.dat not found at {path}")]
    MissingLevelDat { path: PathBuf },
    #[error("cannot read {path}")]
    UnreadableLevelDat {
        path: PathBuf,
        #[source]
        source: NbtError,
    },
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl LayoutError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        LayoutError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Detects the layout of the world or server at `path`.
pub fn detect(path: &Path) -> Result<WorldLayout, LayoutError> {
    let root = fs::canonicalize(path).map_err(|e| LayoutError::io(path, e))?;
    if !root.is_dir() {
        return Err(LayoutError::NotAWorld { path: root });
    }

    let (kind, base) = classify(&root)?;
    let dims = collect_dimensions(&root, kind, base.as_deref())?;
    let main_world_folder = match &base {
        Some(base) => root.join(base),
        None => root.clone(),
    };
    let mut world_folders: Vec<PathBuf> = dims.values().map(|d| d.world_folder.clone()).collect();
    world_folders.push(main_world_folder.clone());
    world_folders.sort();
    world_folders.dedup();

    let level = data::read_level_dat(&main_world_folder)?;
    Ok(WorldLayout {
        server_root: kind.is_bukkit().then(|| root.clone()),
        root,
        kind,
        base,
        main_world_folder,
        world_folders,
        data_version: level.data_version,
        level_spawn_mobs: level.spawn_mobs,
        dimensions: dims.into_values().collect(),
    })
}

fn classify(root: &Path) -> Result<(LayoutKind, Option<String>), LayoutError> {
    let trio = |dir: &Path| -> [PathBuf; 3] {
        let mc = dir.join("dimensions").join("minecraft");
        [
            mc.join("overworld"),
            mc.join("the_nether"),
            mc.join("the_end"),
        ]
    };

    for base in bukkit_bases(root)? {
        let base_dir = root.join(&base);
        if any_world_dir(&trio(&base_dir))? {
            // W-1 for 26.1: a vanilla server root has the same shape; Bukkit-based
            // servers always create bukkit.yml in the server root.
            if !root.join("bukkit.yml").is_file() {
                return Err(LayoutError::LooksLikeVanillaServer {
                    suggested: base_dir,
                });
            }
            return Ok((LayoutKind::Bukkit, Some(base)));
        }
        let nether = root.join(format!("{base}_nether")).join("DIM-1");
        let end = root.join(format!("{base}_the_end")).join("DIM1");
        if any_world_dir(&[base_dir.clone(), nether.clone(), end.clone()])? {
            // W-1: a vanilla server root, whose nether and end live inside the world folder.
            let inner = any_world_dir(&[base_dir.join("DIM-1"), base_dir.join("DIM1")])?;
            if inner && !any_world_dir(&[nether, end])? {
                return Err(LayoutError::LooksLikeVanillaServer {
                    suggested: base_dir,
                });
            }
            return Ok((LayoutKind::BukkitLegacy, Some(base)));
        }
    }

    let kind = if any_world_dir(&trio(root))? {
        LayoutKind::Vanilla
    } else if any_world_dir(&[root.to_path_buf(), root.join("DIM-1"), root.join("DIM1")])? {
        LayoutKind::VanillaLegacy
    } else {
        return Err(LayoutError::NotAWorld {
            path: root.to_path_buf(),
        });
    };

    // W-2: a world folder of a Bukkit-based server.
    let parent_has = |name: &str| root.parent().is_some_and(|p| p.join(name).is_file());
    if root.join("paper-world.yml").is_file()
        || parent_has("bukkit.yml")
        || parent_has("spigot.yml")
    {
        return Err(LayoutError::LooksLikeBukkitWorldFolder);
    }
    Ok((kind, None))
}

/// Candidate Bukkit bases: `level-name` if usable, otherwise elected from the directory structure.
fn bukkit_bases(root: &Path) -> Result<Vec<String>, LayoutError> {
    if let Some(base) = level_name(root) {
        return Ok(vec![base]);
    }
    let mut bases = Vec::new();
    for entry in read_dir_opt(root)? {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !path.is_dir() || matches!(name.as_str(), "dimensions" | "DIM-1" | "DIM1") {
            continue;
        }
        let sibling = |suffix: &str| root.join(format!("{name}{suffix}")).is_dir();
        if sibling("_nether") || sibling("_the_end") || path.join("dimensions").is_dir() {
            bases.push(name);
        }
    }
    bases.sort();
    if let Some(pos) = bases.iter().position(|b| b == "world") {
        let world = bases.remove(pos);
        bases.insert(0, world);
    }
    Ok(bases)
}

/// `level-name` from `server.properties`, if it is a single plain directory name.
fn level_name(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("server.properties")).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("level-name=")?.trim();
        let plain =
            !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\']);
        plain.then(|| value.to_owned())
    })
}

fn any_world_dir(dirs: &[PathBuf]) -> Result<bool, LayoutError> {
    for dir in dirs {
        if is_world_dir(dir)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `dir` holds at least one parseable region file in `region/`, `entities/` or `poi/`.
fn is_world_dir(dir: &Path) -> Result<bool, LayoutError> {
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(LayoutError::io(dir, e)),
    }
    for kind in FileKind::ALL {
        let kind_dir = dir.join(kind.dir_name());
        let entries = match fs::read_dir(&kind_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(LayoutError::io(&kind_dir, e)),
        };
        // Stop at the first region file instead of listing the whole folder.
        for entry in entries {
            let entry = entry.map_err(|e| LayoutError::io(&kind_dir, e))?;
            let is_region = entry
                .file_name()
                .to_str()
                .is_some_and(|n| parse_region_name(n).is_some());
            if is_region && entry.path().is_file() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Lists a directory; a missing directory is empty.
pub(crate) fn read_dir_opt(dir: &Path) -> Result<Vec<fs::DirEntry>, LayoutError> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| LayoutError::io(dir, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(LayoutError::io(dir, e)),
    }
}

fn collect_dimensions(
    root: &Path,
    kind: LayoutKind,
    base: Option<&str>,
) -> Result<BTreeMap<DimensionId, Dimension>, LayoutError> {
    let mut dims = BTreeMap::new();
    let fixed = |dims: &mut BTreeMap<DimensionId, Dimension>,
                 id: DimensionId,
                 dir: PathBuf,
                 world: &Path| {
        if dir.is_dir() {
            dims.insert(
                id.clone(),
                Dimension {
                    id,
                    dir,
                    world_folder: world.to_path_buf(),
                },
            );
        }
    };
    match (kind, base) {
        (LayoutKind::Bukkit, Some(base)) => {
            let world = root.join(base);
            collect_tree(&mut dims, &world.join("dimensions"), &world)?;
        }
        (LayoutKind::BukkitLegacy, Some(base)) => {
            for entry in read_dir_opt(root)? {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Some(suffix) = name.strip_prefix(&format!("{base}_")) else {
                    continue;
                };
                if suffix != "nether" && suffix != "the_end" && entry.path().is_dir() {
                    collect_tree(&mut dims, &entry.path().join("dimensions"), &entry.path())?;
                }
            }
            let world = root.join(base);
            fixed(&mut dims, DimensionId::overworld(), world.clone(), &world);
            let nether = root.join(format!("{base}_nether"));
            fixed(
                &mut dims,
                DimensionId::the_nether(),
                nether.join("DIM-1"),
                &nether,
            );
            let end = root.join(format!("{base}_the_end"));
            fixed(&mut dims, DimensionId::the_end(), end.join("DIM1"), &end);
        }
        (LayoutKind::Vanilla, _) => collect_tree(&mut dims, &root.join("dimensions"), root)?,
        (LayoutKind::VanillaLegacy, _) => {
            collect_tree(&mut dims, &root.join("dimensions"), root)?;
            fixed(
                &mut dims,
                DimensionId::overworld(),
                root.to_path_buf(),
                root,
            );
            fixed(
                &mut dims,
                DimensionId::the_nether(),
                root.join("DIM-1"),
                root,
            );
            fixed(&mut dims, DimensionId::the_end(), root.join("DIM1"), root);
        }
        _ => unreachable!("bukkit layouts always have a base"),
    }
    Ok(dims)
}

/// Adds every `<tree>/<ns>/<name>` directory as dimension `<ns>:<name>`.
fn collect_tree(
    dims: &mut BTreeMap<DimensionId, Dimension>,
    tree: &Path,
    world_folder: &Path,
) -> Result<(), LayoutError> {
    for ns in read_dir_opt(tree)? {
        if !ns.path().is_dir() {
            continue;
        }
        for name in read_dir_opt(&ns.path())? {
            if !name.path().is_dir() {
                continue;
            }
            let (Some(n), Some(m)) = (
                ns.file_name().to_str().map(str::to_owned),
                name.file_name().to_str().map(str::to_owned),
            ) else {
                continue;
            };
            // Directory names that are not valid IDs are not dimensions the game loads.
            if let Ok(id) = DimensionId::parse(&format!("{n}:{m}")) {
                let dim = Dimension {
                    id: id.clone(),
                    dir: name.path(),
                    world_folder: world_folder.to_path_buf(),
                };
                if let Some(first) = dims.insert(id, dim) {
                    // Two Bukkit sibling folders holding the same datapack dimension.
                    return Err(LayoutError::DuplicateDimension {
                        first: first.dir,
                        second: name.path(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_util {
    //! Builders for world folders on disk.

    use std::fs;
    use std::io::Write;
    use std::path::Path;

    use crate::nbt::test_util::{V, doc};

    pub fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(bytes).unwrap();
        gz.finish().unwrap()
    }

    pub fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A region file with an empty header, enough to mark a world directory.
    pub fn region(dir: &Path, name: &str) {
        write(&dir.join("region").join(name), &[0; 8192]);
    }

    pub fn level_dat(world: &Path, data_version: i32) {
        let nbt = doc(vec![(
            "Data",
            V::Compound(vec![("DataVersion", V::Int(data_version))]),
        )]);
        write(&world.join("level.dat"), &gzip(&nbt));
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn ids(layout: &WorldLayout) -> Vec<&str> {
        layout.dimensions.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn vanilla_legacy() {
        let t = tmp();
        let w = t.path();
        region(w, "r.0.0.mca");
        region(&w.join("DIM-1"), "r.0.0.mca");
        level_dat(w, 3700);
        let l = detect(w).unwrap();
        assert_eq!(l.kind, LayoutKind::VanillaLegacy);
        assert_eq!(l.base, None);
        assert_eq!(l.server_root, None);
        assert_eq!(l.data_version, Some(DataVersion(3700)));
        assert_eq!(ids(&l), ["minecraft:overworld", "minecraft:the_nether"]);
        assert_eq!(l.world_folders, std::slice::from_ref(&l.root));
    }

    #[test]
    fn vanilla_26_1() {
        let t = tmp();
        let w = t.path();
        region(&w.join("dimensions/minecraft/overworld"), "r.0.0.mca");
        fs::create_dir_all(w.join("dimensions/minecraft/the_end")).unwrap();
        fs::create_dir_all(w.join("dimensions/my_pack/sky")).unwrap();
        level_dat(w, 5023);
        let l = detect(w).unwrap();
        assert_eq!(l.kind, LayoutKind::Vanilla);
        assert_eq!(
            ids(&l),
            ["minecraft:overworld", "minecraft:the_end", "my_pack:sky"]
        );
    }

    #[test]
    fn bukkit_legacy_with_custom_dimension() {
        let t = tmp();
        let r = t.path();
        write(&r.join("server.properties"), b"#c\nlevel-name= survival \n");
        region(&r.join("survival"), "r.0.0.mca");
        region(&r.join("survival_nether/DIM-1"), "r.0.0.mca");
        region(
            &r.join("survival_my_pack_sky_islands/dimensions/my_pack/sky_islands"),
            "r.0.0.mca",
        );
        level_dat(&r.join("survival"), 4325);
        let l = detect(r).unwrap();
        assert_eq!(l.kind, LayoutKind::BukkitLegacy);
        assert_eq!(l.base.as_deref(), Some("survival"));
        assert_eq!(l.server_root.as_deref(), Some(l.root.as_path()));
        assert_eq!(l.main_world_folder, l.root.join("survival"));
        assert_eq!(
            ids(&l),
            [
                "minecraft:overworld",
                "minecraft:the_nether",
                "my_pack:sky_islands"
            ]
        );
        let nether = &l.dimensions[1];
        assert_eq!(nether.world_folder, l.root.join("survival_nether"));
        assert_eq!(
            l.world_folders,
            [
                "survival",
                "survival_my_pack_sky_islands",
                "survival_nether"
            ]
            .map(|n| l.root.join(n))
        );
    }

    #[test]
    fn bukkit_26_1_without_level_name() {
        let t = tmp();
        let r = t.path();
        write(&r.join("bukkit.yml"), b"");
        region(
            &r.join("world/dimensions/minecraft/the_nether"),
            "r.0.0.mca",
        );
        level_dat(&r.join("world"), 5023);
        let l = detect(r).unwrap();
        assert_eq!(l.kind, LayoutKind::Bukkit);
        assert_eq!(l.base.as_deref(), Some("world"));
        assert_eq!(ids(&l), ["minecraft:the_nether"]);
        assert_eq!(l.world_folders, [l.root.join("world")]);
    }

    #[test]
    fn unusable_level_name_is_ignored() {
        for value in ["../x", "", "/abs", ".", "a/b"] {
            let t = tmp();
            write(
                &t.path().join("server.properties"),
                format!("level-name={value}\n").as_bytes(),
            );
            assert_eq!(level_name(t.path()), None, "{value:?}");
        }
    }

    #[test]
    fn w1_vanilla_server_root() {
        let t = tmp();
        let r = t.path();
        write(&r.join("server.properties"), b"level-name=world\n");
        region(&r.join("world"), "r.0.0.mca");
        region(&r.join("world/DIM-1"), "r.0.0.mca");
        match detect(r) {
            Err(LayoutError::LooksLikeVanillaServer { suggested }) => {
                assert_eq!(suggested, fs::canonicalize(r).unwrap().join("world"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn w1_vanilla_26_1_server_root() {
        let t = tmp();
        let r = t.path();
        write(&r.join("server.properties"), b"level-name=world\n");
        region(&r.join("world/dimensions/minecraft/overworld"), "r.0.0.mca");
        assert!(matches!(
            detect(r),
            Err(LayoutError::LooksLikeVanillaServer { .. })
        ));
    }

    #[test]
    fn duplicate_datapack_dimension_is_an_error() {
        let t = tmp();
        let r = t.path();
        write(&r.join("server.properties"), b"level-name=world\n");
        region(&r.join("world"), "r.0.0.mca");
        level_dat(&r.join("world"), 4325);
        region(&r.join("world_a/dimensions/pack/sky"), "r.0.0.mca");
        region(&r.join("world_b/dimensions/pack/sky"), "r.0.0.mca");
        assert!(matches!(
            detect(r),
            Err(LayoutError::DuplicateDimension { .. })
        ));
    }

    #[test]
    fn w2_bukkit_world_folder() {
        let t = tmp();
        let r = t.path();
        write(&r.join("bukkit.yml"), b"");
        region(&r.join("world"), "r.0.0.mca");
        level_dat(&r.join("world"), 4325);
        assert!(matches!(
            detect(&r.join("world")),
            Err(LayoutError::LooksLikeBukkitWorldFolder)
        ));
    }

    #[test]
    fn not_a_world_and_missing_level_dat() {
        let t = tmp();
        assert!(matches!(
            detect(t.path()),
            Err(LayoutError::NotAWorld { .. })
        ));
        region(t.path(), "r.0.0.mca");
        assert!(matches!(
            detect(t.path()),
            Err(LayoutError::MissingLevelDat { .. })
        ));
    }
}
