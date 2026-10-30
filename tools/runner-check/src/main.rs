//! Six closed Cargo checks. Runner supplies a read-only checkout and private storage.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, File},
    io::{self, Read},
    os::unix::{fs::DirBuilderExt, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode},
};

mod registry;
mod sha256;

const SCRATCH: &str = "/run/agent-runner/scratch";
const CACHE: &str = "/run/agent-runner/cache";
type Result<T> = io::Result<T>;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    ClippyWorkspace,
    DocWorkspace,
    TestWorkspace,
    ClippyHelper,
    DocHelper,
    TestHelper,
}

impl Mode {
    fn parse(value: &OsStr) -> Result<Self> {
        match value.to_str() {
            Some("clippy-workspace") => Ok(Self::ClippyWorkspace),
            Some("doc-workspace") => Ok(Self::DocWorkspace),
            Some("test-workspace") => Ok(Self::TestWorkspace),
            Some("clippy-helper") => Ok(Self::ClippyHelper),
            Some("doc-helper") => Ok(Self::DocHelper),
            Some("test-helper") => Ok(Self::TestHelper),
            _ => Err(invalid("unknown check mode")),
        }
    }

    fn helper(self) -> bool {
        matches!(
            self,
            Self::ClippyHelper | Self::DocHelper | Self::TestHelper
        )
    }

    fn doc(self) -> bool {
        matches!(self, Self::DocWorkspace | Self::DocHelper)
    }

    fn arguments(self, target: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![match self {
            Self::ClippyWorkspace | Self::ClippyHelper => "clippy".into(),
            Self::DocWorkspace | Self::DocHelper => "doc".into(),
            Self::TestWorkspace | Self::TestHelper => "test".into(),
        }];
        if self.helper() {
            args.extend([
                "--manifest-path".into(),
                "tools/runner-check/Cargo.toml".into(),
            ]);
        }
        args.extend(["--locked".into(), "--workspace".into()]);
        if matches!(self, Self::ClippyWorkspace | Self::ClippyHelper) {
            args.push("--all-targets".into());
        } else if self.doc() {
            args.extend(["--no-deps".into(), "--document-private-items".into()]);
        }
        args.extend(["--offline".into(), "--target-dir".into(), target.into()]);
        if matches!(self, Self::ClippyWorkspace | Self::ClippyHelper) {
            args.extend(["--".into(), "-D".into(), "warnings".into()]);
        }
        args
    }
}

struct Options {
    mode: Mode,
    toolchain: PathBuf,
    registry: PathBuf,
}

impl Options {
    fn parse(args: &[OsString]) -> Result<Self> {
        if args.len() != 5 || args[1] != "--toolchain-bin" || args[3] != "--registry-source" {
            return Err(invalid(
                "expected MODE --toolchain-bin ABSOLUTE --registry-source RELATIVE",
            ));
        }
        let options = Self {
            mode: Mode::parse(&args[0])?,
            toolchain: args[2].clone().into(),
            registry: args[4].clone().into(),
        };
        if !options.toolchain.is_absolute()
            || options
                .toolchain
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(invalid("toolchain must be an absolute bin directory"));
        }
        relative(&options.registry)?;
        Ok(options)
    }
}

fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("expected a confined relative path"));
    }
    Ok(())
}

/// All input ancestors are checked. Runner keeps these inputs immutable while owned.
fn checked_path(root: &Path, relative_path: &Path, directory: bool) -> Result<PathBuf> {
    relative(relative_path)?;
    let mut path = root.to_path_buf();
    for component in relative_path.components() {
        path.push(component);
        let kind = fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(invalid("linked or special input"));
        }
    }
    let metadata = fs::symlink_metadata(&path)?;
    if directory != metadata.is_dir() {
        return Err(invalid("unexpected input type"));
    }
    if metadata.is_file() {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(invalid("multiply linked input"));
        }
    }
    Ok(path)
}

fn absolute_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid("expected an absolute directory"));
    }
    checked_path(
        Path::new("/"),
        path.strip_prefix("/")
            .map_err(|_| invalid("invalid root"))?,
        true,
    )?;
    Ok(())
}

/// Exclusive mode-0700 directories, also used by the helper's file-only tests.
struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    fn new(parent: &Path) -> Result<Self> {
        absolute_directory(parent)?;
        let mut random = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let path = parent.join(format!("runner-check-{:032x}", u128::from_ne_bytes(random)));
        DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(options: &Options, checkout: &Path, scratch: &Path, home: &Path) -> Result<Command> {
    let mut command = Command::new(options.toolchain.join("cargo"));
    // Use the exposed actual toolchain, never rustup shims or inherited Cargo overrides.
    command
        .current_dir(checkout)
        .env_clear()
        .env("HOME", "/nonexistent")
        .env(
            "PATH",
            env::join_paths([
                options.toolchain.as_path(),
                Path::new("/usr/bin"),
                Path::new("/bin"),
            ])
            .map_err(|_| invalid("toolchain cannot form a PATH entry"))?,
        )
        .env("RUSTC", options.toolchain.join("rustc"))
        .env("RUSTDOC", options.toolchain.join("rustdoc"))
        .env("CARGO_HOME", home)
        .env("CARGO_NET_OFFLINE", "true")
        .env("TMPDIR", scratch.join("tmp"))
        .env("XDG_CACHE_HOME", home.join("xdg"))
        .env("LC_ALL", "C")
        .args(options.mode.arguments(&scratch.join("target")));
    if options.mode.doc() {
        command.env("RUSTDOCFLAGS", "-D warnings");
    }
    Ok(command)
}

struct PreparedCheck {
    command: Command,
    _scratch: PrivateDirectory,
    _home: PrivateDirectory,
}

fn prepare(
    options: &Options,
    checkout: &Path,
    scratch_root: &Path,
    cache_root: &Path,
) -> Result<PreparedCheck> {
    absolute_directory(checkout)?;
    let source = checked_path(checkout, &options.registry, true)?;
    let lock = if options.mode.helper() {
        "tools/runner-check/Cargo.lock"
    } else {
        "Cargo.lock"
    };
    let packages = registry::packages(&fs::read_to_string(checked_path(
        checkout,
        Path::new(lock),
        false,
    )?)?)?;
    let scratch = PrivateDirectory::new(scratch_root)?;
    let home = PrivateDirectory::new(cache_root)?;
    DirBuilder::new()
        .mode(0o700)
        .create(scratch.0.join("tmp"))?;
    registry::provision(&source, &home.0, &packages)?;
    Ok(PreparedCheck {
        command: command(options, checkout, &scratch.0, &home.0)?,
        _scratch: scratch,
        _home: home,
    })
}

fn run() -> Result<()> {
    let options = Options::parse(&env::args_os().skip(1).collect::<Vec<_>>())?;
    absolute_directory(&options.toolchain)?;
    // The actual cargo executable already exposes this toolchain root to the sandbox.
    for name in ["cargo", "rustc", "rustdoc", "cargo-clippy", "clippy-driver"] {
        use std::os::unix::fs::PermissionsExt;
        let executable = checked_path(&options.toolchain, Path::new(name), false)?;
        if fs::metadata(executable)?.permissions().mode() & 0o111 == 0 {
            return Err(invalid("toolchain executable is not executable"));
        }
    }
    for (key, value) in [
        ("AGENT_RUNNER_SCRATCH", SCRATCH),
        ("AGENT_RUNNER_CACHE", CACHE),
    ] {
        if env::var_os(key).as_deref() != Some(OsStr::new(value)) {
            return Err(invalid("declared Runner storage is required"));
        }
    }
    let mut prepared = prepare(
        &options,
        &env::current_dir()?,
        Path::new(SCRATCH),
        Path::new(CACHE),
    )?;
    // exec preserves every Cargo exit code and signal. Runner retires descendants and then
    // removes its private scratch/cache; successful exec deliberately bypasses our Drop.
    Err(prepared.command.exec())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // OS diagnostics can contain host paths; expose only the error classification.
            eprintln!("runner-check: setup or exec failed ({:?})", error.kind());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
