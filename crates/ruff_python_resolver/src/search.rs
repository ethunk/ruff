//! Determine the appropriate search paths for the Python environment.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use log::debug;

use crate::config::Config;
use crate::module_descriptor::ImportModuleDescriptor;
use crate::python_version::PythonVersion;
use crate::{host, SITE_PACKAGES};

/// Find the `site-packages` directory for the specified Python version.
fn find_site_packages_path(
    lib_path: &Path,
    python_version: Option<PythonVersion>,
) -> Option<PathBuf> {
    if lib_path.is_dir() {
        debug!(
            "Found path `{}`; looking for site-packages",
            lib_path.display()
        );
    } else {
        debug!("Did not find `{}`", lib_path.display());
    }

    let site_packages_path = lib_path.join(SITE_PACKAGES);
    if site_packages_path.is_dir() {
        debug!("Found path `{}`", site_packages_path.display());
        return Some(site_packages_path);
    }

    debug!(
        "Did not find `{}`, so looking for Python subdirectory",
        site_packages_path.display()
    );

    // There's no `site-packages` directory in the library directory; look for a `python3.X`
    // directory instead.
    let candidate_dirs: Vec<PathBuf> = fs::read_dir(lib_path)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = entry.metadata().ok()?;

            if metadata.file_type().is_dir() {
                let dir_path = entry.path();
                if dir_path
                    .file_name()
                    .and_then(OsStr::to_str)?
                    .starts_with("python3.")
                {
                    if dir_path.join(SITE_PACKAGES).is_dir() {
                        return Some(dir_path);
                    }
                }
            } else if metadata.file_type().is_symlink() {
                let symlink_path = fs::read_link(entry.path()).ok()?;
                if symlink_path
                    .file_name()
                    .and_then(OsStr::to_str)?
                    .starts_with("python3.")
                {
                    if symlink_path.join(SITE_PACKAGES).is_dir() {
                        return Some(symlink_path);
                    }
                }
            }

            None
        })
        .collect();

    // If a `python3.X` directory does exist (and `3.X` matches the current Python version),
    // prefer it over any other Python directories.
    if let Some(python_version) = python_version {
        if let Some(preferred_dir) = candidate_dirs.iter().find(|dir| {
            dir.file_name()
                .and_then(OsStr::to_str)
                .map_or(false, |name| name == python_version.dir())
        }) {
            debug!("Found path `{}`", preferred_dir.display());
            return Some(preferred_dir.join(SITE_PACKAGES));
        }
    }

    // Fallback to the first `python3.X` directory that we found.
    let default_dir = candidate_dirs.first()?;
    debug!("Found path `{}`", default_dir.display());
    Some(default_dir.join(SITE_PACKAGES))
}

fn get_paths_from_pth_files(parent_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(parent_dir)
        .unwrap()
        .flatten()
        .filter(|entry| {
            // Collect all *.pth files.
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            file_type.is_file() || file_type.is_symlink()
        })
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("pth")))
        .filter(|path| {
            // Skip all files that are much larger than expected.
            let Ok(metadata) = path.metadata() else {
                return false;
            };
            let file_len = metadata.len();
            file_len > 0 && file_len < 64 * 1024
        })
        .filter_map(|path| {
            let data = fs::read_to_string(&path).ok()?;
            for line in data.lines() {
                let trimmed_line = line.trim();
                if !trimmed_line.is_empty()
                    && !trimmed_line.starts_with('#')
                    && !trimmed_line.starts_with("import")
                {
                    let pth_path = parent_dir.join(trimmed_line);
                    if pth_path.is_dir() {
                        return Some(pth_path);
                    }
                }
            }
            None
        })
        .collect()
}

/// Find the Python search paths for the given virtual environment.
pub(crate) fn find_python_search_paths<Host: host::Host>(
    config: &Config,
    host: &Host,
) -> Vec<PathBuf> {
    if let Some(venv_path) = config.venv_path.as_ref() {
        if let Some(venv) = config.venv.as_ref() {
            let mut found_paths = vec![];

            for lib_name in ["lib", "Lib", "lib64"] {
                let lib_path = venv_path.join(venv).join(lib_name);
                if let Some(site_packages_path) =
                    find_site_packages_path(&lib_path, config.default_python_version)
                {
                    // Add paths from any `.pth` files in each of the `site-packages` directories.
                    found_paths.extend(get_paths_from_pth_files(&site_packages_path));

                    // Add the `site-packages` directory to the search path.
                    found_paths.push(site_packages_path);
                }
            }

            if !found_paths.is_empty() {
                found_paths.sort();
                found_paths.dedup();

                debug!("Found the following `site-packages` dirs");
                for path in &found_paths {
                    debug!("  {}", path.display());
                }

                return found_paths;
            }
        }
    }

    // Fall back to the Python interpreter.
    host.python_search_paths()
}

/// Determine the relevant Python search paths.
fn get_python_search_paths<Host: host::Host>(config: &Config, host: &Host) -> Vec<PathBuf> {
    // TODO(charlie): Cache search paths.
    find_python_search_paths(config, host)
}

/// Determine the root of the `typeshed` directory.
pub(crate) fn get_typeshed_root<Host: host::Host>(config: &Config, host: &Host) -> Option<PathBuf> {
    if let Some(typeshed_path) = config.typeshed_path.as_ref() {
        // Did the user specify a typeshed path?
        if typeshed_path.is_dir() {
            return Some(typeshed_path.clone());
        }
    } else {
        // If not, we'll look in the Python search paths.
        for python_search_path in get_python_search_paths(config, host) {
            let possible_typeshed_path = python_search_path.join("typeshed");
            if possible_typeshed_path.is_dir() {
                return Some(possible_typeshed_path);
            }
        }
    }

    None
}

/// Format the expected `typeshed` subdirectory.
fn format_typeshed_subdirectory(typeshed_path: &Path, is_stdlib: bool) -> PathBuf {
    typeshed_path.join(if is_stdlib { "stdlib" } else { "stubs" })
}

/// Determine the current `typeshed` subdirectory.
fn get_typeshed_subdirectory<Host: host::Host>(
    is_stdlib: bool,
    config: &Config,
    host: &Host,
) -> Option<PathBuf> {
    let typeshed_path = get_typeshed_root(config, host)?;
    let typeshed_path = format_typeshed_subdirectory(&typeshed_path, is_stdlib);
    if typeshed_path.is_dir() {
        Some(typeshed_path)
    } else {
        None
    }
}

/// Determine the current `typeshed` subdirectory for the standard library.
pub(crate) fn get_stdlib_typeshed_path<Host: host::Host>(
    config: &Config,
    host: &Host,
) -> Option<PathBuf> {
    get_typeshed_subdirectory(true, config, host)
}

/// Generate a map from PyPI-registered package name to a list of paths
/// containing the package's stubs.
fn build_typeshed_third_party_package_map(third_party_dir: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut package_map = HashMap::new();

    // Iterate over every directory.
    for outer_entry in fs::read_dir(third_party_dir).unwrap() {
        let outer_entry = outer_entry.unwrap();
        if outer_entry.file_type().unwrap().is_dir() {
            // Iterate over any subdirectory children.
            for inner_entry in fs::read_dir(outer_entry.path()).unwrap() {
                let inner_entry = inner_entry.unwrap();

                if inner_entry.file_type().unwrap().is_dir() {
                    package_map
                        .entry(inner_entry.file_name().to_string_lossy().to_string())
                        .or_insert_with(Vec::new)
                        .push(outer_entry.path());
                } else if inner_entry.file_type().unwrap().is_file() {
                    if inner_entry
                        .path()
                        .extension()
                        .map_or(false, |extension| extension == "pyi")
                    {
                        let stripped_file_name = inner_entry
                            .path()
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                            .to_string();
                        package_map
                            .entry(stripped_file_name)
                            .or_insert_with(Vec::new)
                            .push(outer_entry.path());
                    }
                }
            }
        }
    }

    package_map
}

/// Determine the current `typeshed` subdirectory for a third-party package.
pub(crate) fn get_third_party_typeshed_package_paths<Host: host::Host>(
    module_descriptor: &ImportModuleDescriptor,
    config: &Config,
    host: &Host,
) -> Option<Vec<PathBuf>> {
    let typeshed_path = get_typeshed_subdirectory(false, config, host)?;
    let package_paths = build_typeshed_third_party_package_map(&typeshed_path);
    let first_name_part = module_descriptor.name_parts.first().map(String::as_str)?;
    package_paths.get(first_name_part).cloned()
}
