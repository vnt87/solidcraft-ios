//! `cargo xtask ios`: build the SolidCraft iPad bundle and run it on the Simulator or a device.
//!
//! Every decision is pure: [`parse`] turns arguments into [`Options`], [`plan`] turns options into an
//! ordered [`Plan`] of command specs, and [`execute`] is a thin runner. The Xcode project
//! (`packaging/ios/SolidCraft.xcodeproj`) already runs `build-rust.sh` and `link-rust.sh` as build
//! phases, so the plan only drives `xcodebuild` and does not call cargo for the Rust library itself.
//! The plan can be printed on any host (`--dry-run`); running it needs macOS with Xcode.

use std::path::Path;
use std::process::Command;

pub const USAGE: &str = "\
usage: cargo xtask ios (--sim | --device) [--release] [options]

  --sim               build for the iOS Simulator, boot an iPad simulator, install and launch
  --device            build for a physical iPad (automatic signing), install and launch it
  --release           Release configuration (default: Debug)
  --dry-run           print the commands that would run and exit (works on any host)
  --sim-name NAME     simulator to boot (default: \"iPad Air 13-inch (M3)\")
  --show-simulator    open the desktop Simulator viewer (requires the full Xcode app)
  --screenshot PATH   --sim: after launching, wait for the first frames and save a PNG there
  --device-id ID      device identifier for --device (see `xcrun devicectl list devices`);
                      also read from $SOLIDCRAFT_IOS_DEVICE
  --team ID           Apple development team id for signing (DEVELOPMENT_TEAM);
                      also read from $SOLIDCRAFT_IOS_TEAM
  --bundle-id ID      override the bundle identifier (PRODUCT_BUNDLE_IDENTIFIER) for the build and
                      the launch, e.g. a team-unique development id when the canonical one cannot
                      be registered to your team; also read from $SOLIDCRAFT_IOS_BUNDLE_ID
  -h, --help          print this help

--sim and --device need macOS with Xcode and the Rust targets aarch64-apple-ios,
aarch64-apple-ios-sim (and x86_64-apple-ios). See packaging/ios/README.md.
";

pub const BUNDLE_ID: &str = "io.tangencylabs.solidcraft";
const PROJECT: &str = "packaging/ios/SolidCraft.xcodeproj";
const SCHEME: &str = "SolidCraft";
const DERIVED_DATA: &str = "target/ios-derived";
// UNSURE: the exact simulator name depends on the installed Xcode runtime; override with --sim-name.
const DEFAULT_SIM: &str = "iPad Air 13-inch (M3)";
/// Seconds between launch and `--screenshot`: SolidCraft compiles the viewport's Metal pipelines
/// and draws its Home cards on the first frames, which takes longer than laying out a window.
const SCREENSHOT_DELAY: &str = "12";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sim,
    Device,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    pub release: bool,
    pub dry_run: bool,
    pub sim_name: String,
    pub show_simulator: bool,
    pub screenshot: Option<String>,
    pub device_id: Option<String>,
    pub team: Option<String>,
    /// Bundle id override; `None` keeps the project's canonical [`BUNDLE_ID`].
    pub bundle_id: Option<String>,
}

/// Result of argument parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Options),
}

/// Environment fallbacks for [`parse`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Env<'a> {
    pub device: Option<&'a str>,
    pub team: Option<&'a str>,
    pub bundle_id: Option<&'a str>,
}

/// Parse the arguments after `ios`. `env` holds the environment fallbacks.
pub fn parse(args: &[&str], env: Env) -> Result<Parsed, String> {
    let (mut sim, mut device, mut release, mut dry_run) = (false, false, false, false);
    let mut show_simulator = false;
    let (mut sim_name, mut screenshot, mut device_id, mut team, mut bundle_id) = (None, None, None, None, None);
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "-h" | "--help" | "help" => return Ok(Parsed::Help),
            "--sim" => sim = true,
            "--device" => device = true,
            "--release" => release = true,
            "--dry-run" => dry_run = true,
            "--show-simulator" => show_simulator = true,
            "--sim-name" | "--screenshot" | "--device-id" | "--team" | "--bundle-id" => {
                let v = match it.next() {
                    Some(v) if !v.starts_with('-') && !v.is_empty() => v.to_string(),
                    _ => return Err(format!("{a} needs a value")),
                };
                match a {
                    "--sim-name" => sim_name = Some(v),
                    "--screenshot" => screenshot = Some(v),
                    "--device-id" => device_id = Some(v),
                    "--bundle-id" => bundle_id = Some(v),
                    _ => team = Some(v),
                }
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let mode = match (sim, device) {
        (true, true) => return Err("--sim and --device are mutually exclusive".into()),
        (false, false) => return Err("choose a mode: --sim or --device".into()),
        (true, false) => Mode::Sim,
        (false, true) => Mode::Device,
    };
    if mode == Mode::Sim && (device_id.is_some() || team.is_some()) {
        return Err("--device-id and --team only apply to --device".into());
    }
    if mode == Mode::Device && sim_name.is_some() {
        return Err("--sim-name only applies to --sim".into());
    }
    if mode == Mode::Device && show_simulator {
        return Err("--show-simulator only applies to --sim".into());
    }
    if mode == Mode::Device && screenshot.is_some() {
        return Err("--screenshot only applies to --sim".into());
    }
    let clean = |s: Option<&str>| s.filter(|v| !v.trim().is_empty()).map(str::to_string);
    Ok(Parsed::Run(Options {
        mode,
        release,
        dry_run,
        sim_name: sim_name.unwrap_or_else(|| DEFAULT_SIM.to_string()),
        show_simulator,
        screenshot,
        device_id: device_id.or_else(|| clean(env.device)),
        team: team.or_else(|| clean(env.team)),
        bundle_id: bundle_id.or_else(|| clean(env.bundle_id)),
    }))
}

/// One external command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// A failure is tolerated (e.g. booting an already booted simulator).
    pub allow_failure: bool,
}

impl Step {
    fn new(program: &str, args: &[&str]) -> Self {
        Step { program: program.into(), args: args.iter().map(|s| s.to_string()).collect(), env: Vec::new(), allow_failure: false }
    }

    fn tolerant(mut self) -> Self {
        self.allow_failure = true;
        self
    }

    /// Shell-like rendering for logs and `--dry-run`.
    pub fn display(&self) -> String {
        let quote = |s: &str| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c)) {
                s.to_string()
            } else {
                format!("'{}'", s.replace('\'', "'\\''"))
            }
        };
        let mut parts: Vec<String> = self.env.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
        parts.push(quote(&self.program));
        parts.extend(self.args.iter().map(|a| quote(a)));
        parts.join(" ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Advice printed after the plan (not a command).
    pub notes: Vec<String>,
}

/// The bundle id the build is signed with and the launch step targets.
pub fn bundle_id(o: &Options) -> &str {
    o.bundle_id.as_deref().unwrap_or(BUNDLE_ID)
}

pub fn configuration(o: &Options) -> &'static str {
    if o.release { "Release" } else { "Debug" }
}

/// Where xcodebuild leaves `SolidCraft.app` (relative to the repository root).
pub fn app_path(o: &Options) -> String {
    let platform = match o.mode {
        Mode::Sim => "iphonesimulator",
        Mode::Device => "iphoneos",
    };
    format!("{DERIVED_DATA}/Build/Products/{}-{platform}/{SCHEME}.app", configuration(o))
}

/// Build the ordered command list for `o`. Pure: no I/O.
pub fn plan(o: &Options) -> Plan {
    // A connected device is targeted by id so automatic provisioning can register it. The
    // simulator is named so Xcode builds only this Mac's architecture (ONLY_ACTIVE_ARCH).
    let destination = match (o.mode, &o.device_id) {
        (Mode::Sim, _) => format!("platform=iOS Simulator,name={}", o.sim_name),
        (Mode::Device, Some(id)) => format!("id={id}"),
        (Mode::Device, None) => "generic/platform=iOS".to_string(),
    };
    let bundle = bundle_id(o);
    // xcodebuild runs packaging/ios/build-rust.sh (cargo build --lib per arch + lipo) and
    // link-rust.sh as build phases, so no separate cargo step is needed.
    let mut xcodebuild = Step::new(
        "xcodebuild",
        &["-project", PROJECT, "-scheme", SCHEME, "-configuration", configuration(o), "-destination", &destination, "-derivedDataPath", DERIVED_DATA],
    );
    if o.mode == Mode::Device {
        // Automatic signing is the project default (CODE_SIGN_STYLE = Automatic). UNSURE: whether
        // -allowProvisioningUpdates is needed on the author's machine; it requires an Apple ID in Xcode.
        xcodebuild.args.push("-allowProvisioningUpdates".into());
        if o.device_id.is_some() {
            xcodebuild.args.push("-allowProvisioningDeviceRegistration".into());
        }
        if let Some(team) = &o.team {
            xcodebuild.args.push(format!("DEVELOPMENT_TEAM={team}"));
        }
    }
    if let Some(id) = &o.bundle_id {
        xcodebuild.args.push(format!("PRODUCT_BUNDLE_IDENTIFIER={id}"));
    }
    xcodebuild.args.push("build".into());
    let app = app_path(o);
    let mut steps = vec![xcodebuild];
    let mut notes = Vec::new();
    match o.mode {
        Mode::Sim => {
            // `simctl boot` fails when the device is already booted: tolerated, then wait for it.
            steps.push(Step::new("xcrun", &["simctl", "boot", &o.sim_name]).tolerant());
            steps.push(Step::new("xcrun", &["simctl", "bootstatus", &o.sim_name, "-b"]));
            if o.show_simulator {
                steps.push(Step::new("open", &["-a", "Simulator"]));
            }
            steps.push(Step::new("xcrun", &["simctl", "install", &o.sim_name, &app]));
            // A previous run is replaced, so `--sim` works repeatedly without a manual quit.
            steps.push(Step::new("xcrun", &["simctl", "launch", "--terminate-running-process", &o.sim_name, bundle]));
            if let Some(path) = &o.screenshot {
                steps.push(Step::new("sleep", &[SCREENSHOT_DELAY]));
                steps.push(Step::new("xcrun", &["simctl", "io", &o.sim_name, "screenshot", path]));
            }
        }
        Mode::Device => match &o.device_id {
            Some(id) => {
                // devicectl ships with Xcode 15+. UNSURE of flag spelling on the user's Xcode:
                // `devicectl device install app --device <id> <path>` and
                // `devicectl device process launch --device <id> <bundle id>`.
                steps.push(Step::new("xcrun", &["devicectl", "device", "install", "app", "--device", id, &app]));
                steps.push(Step::new("xcrun", &["devicectl", "device", "process", "launch", "--device", id, bundle]));
            }
            None => {
                steps.push(Step::new("xcrun", &["devicectl", "list", "devices"]));
                notes.push("no device id: built only. Pass --device-id <id> (or set SOLIDCRAFT_IOS_DEVICE) from the list above to install and launch.".into());
            }
        },
    }
    Plan { steps, notes }
}

/// Hosts that can run Xcode tooling.
pub fn host_supported(os: &str) -> bool {
    os == "macos"
}

pub fn refusal(os: &str) -> String {
    format!("`xtask ios --sim/--device` needs macOS with Xcode (this host is {os}); use --dry-run to print the plan")
}

pub fn render(plan: &Plan) -> String {
    let mut out = String::new();
    for (i, s) in plan.steps.iter().enumerate() {
        out.push_str(&format!("{}. {}{}\n", i + 1, s.display(), if s.allow_failure { "   # failure tolerated" } else { "" }));
    }
    for n in &plan.notes {
        out.push_str(&format!("note: {n}\n"));
    }
    out
}

/// Thin executor: runs each step from `root`, stopping at the first non-tolerated failure.
fn execute(plan: &Plan, root: &Path) -> Result<(), String> {
    for s in &plan.steps {
        eprintln!("$ {}", s.display());
        let mut c = Command::new(&s.program);
        c.args(&s.args).envs(s.env.iter().map(|(k, v)| (k, v))).current_dir(root);
        let status = c.status().map_err(|e| format!("{}: failed to spawn: {e}", s.program))?;
        if !status.success() && !s.allow_failure {
            return Err(format!("`{}` exited with {status}", s.display()));
        }
    }
    for n in &plan.notes {
        eprintln!("note: {n}");
    }
    Ok(())
}

/// Entry point for `cargo xtask ios`.
pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let env_device = std::env::var("SOLIDCRAFT_IOS_DEVICE").ok();
    let env_team = std::env::var("SOLIDCRAFT_IOS_TEAM").ok();
    let env_bundle = std::env::var("SOLIDCRAFT_IOS_BUNDLE_ID").ok();
    let env = Env { device: env_device.as_deref(), team: env_team.as_deref(), bundle_id: env_bundle.as_deref() };
    let o = match parse(args, env)? {
        Parsed::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Parsed::Run(o) => o,
    };
    let p = plan(&o);
    if o.dry_run {
        print!("{}", render(&p));
        return Ok(());
    }
    if !host_supported(std::env::consts::OS) {
        return Err(refusal(std::env::consts::OS));
    }
    execute(&p, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(args: &[&str]) -> Options {
        match parse(args, Env::default()) {
            Ok(Parsed::Run(o)) => o,
            other => panic!("expected options, got {other:?}"),
        }
    }

    #[test]
    fn help_variants() {
        for a in ["--help", "-h", "help"] {
            assert_eq!(parse(&[a], Env::default()), Ok(Parsed::Help));
        }
        assert_eq!(parse(&["--sim", "--help"], Env::default()), Ok(Parsed::Help));
    }

    #[test]
    fn parse_errors() {
        let err = |a: &[&str]| parse(a, Env::default()).unwrap_err();
        assert!(err(&[]).contains("choose a mode"));
        assert!(err(&["--release"]).contains("choose a mode"));
        assert!(err(&["--sim", "--device"]).contains("mutually exclusive"));
        assert!(err(&["--sim", "--bogus"]).contains("unknown argument `--bogus`"));
        assert!(err(&["--sim", "--sim-name"]).contains("needs a value"));
        assert!(err(&["--sim", "--sim-name", "--release"]).contains("needs a value"));
        assert!(err(&["--sim", "--device-id", "X"]).contains("only apply to --device"));
        assert!(err(&["--sim", "--team", "T"]).contains("only apply to --device"));
        assert!(err(&["--device", "--sim-name", "iPad"]).contains("only applies to --sim"));
        assert!(err(&["--device", "--show-simulator"]).contains("only applies to --sim"));
        assert!(err(&["--device", "--screenshot", "a.png"]).contains("only applies to --sim"));
        assert!(err(&["--sim", "--bundle-id"]).contains("needs a value"));
        assert!(err(&["--sim", "--screenshot"]).contains("needs a value"));
    }

    #[test]
    fn parse_options_and_env() {
        let o = opts(&["--sim", "--release", "--dry-run"]);
        assert_eq!((o.mode, o.release, o.dry_run), (Mode::Sim, true, true));
        assert_eq!(o.sim_name, DEFAULT_SIM);
        let Ok(Parsed::Run(o)) = parse(&["--device"], Env { device: Some("ENVDEV"), team: Some("ENVTEAM"), bundle_id: Some("env.bundle") }) else { panic!() };
        assert_eq!((o.device_id.as_deref(), o.team.as_deref(), o.bundle_id.as_deref()), (Some("ENVDEV"), Some("ENVTEAM"), Some("env.bundle")));
        let Ok(Parsed::Run(o)) = parse(&["--device", "--device-id", "CLI", "--team", "T1"], Env { device: Some("ENVDEV"), ..Env::default() }) else { panic!() };
        assert_eq!((o.device_id.as_deref(), o.team.as_deref()), (Some("CLI"), Some("T1")));
        let Ok(Parsed::Run(o)) = parse(&["--device"], Env { device: Some("  "), team: Some(""), bundle_id: Some(" ") }) else { panic!() };
        assert_eq!((o.device_id, o.team, o.bundle_id), (None, None, None));
    }

    #[test]
    fn sim_launch_does_not_require_desktop_viewer() {
        let p = plan(&opts(&["--sim"]));
        assert!(!p.steps.iter().any(|step| step.program == "open"));
        assert_eq!(p.steps.last().expect("launch step").args, ["simctl", "launch", "--terminate-running-process", DEFAULT_SIM, BUNDLE_ID]);
    }

    #[test]
    fn sim_plan() {
        let p = plan(&opts(&["--sim"]));
        let lines: Vec<String> = p.steps.iter().map(Step::display).collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("xcodebuild -project packaging/ios/SolidCraft.xcodeproj -scheme SolidCraft -configuration Debug"));
        assert!(lines[0].contains("-destination 'platform=iOS Simulator,name=iPad Air 13-inch (M3)'"));
        assert!(lines[0].contains("-derivedDataPath target/ios-derived"));
        assert!(lines[0].ends_with(" build"));
        assert!(!lines[0].contains("allowProvisioningUpdates"));
        assert_eq!(lines[1], "xcrun simctl boot 'iPad Air 13-inch (M3)'");
        assert!(p.steps[1].allow_failure && !p.steps[0].allow_failure);
        assert!(lines[2].contains("bootstatus") && lines[2].ends_with(" -b"));
        assert_eq!(lines[3], "xcrun simctl install 'iPad Air 13-inch (M3)' target/ios-derived/Build/Products/Debug-iphonesimulator/SolidCraft.app");
        assert_eq!(lines[4], "xcrun simctl launch --terminate-running-process 'iPad Air 13-inch (M3)' io.tangencylabs.solidcraft");
    }

    #[test]
    fn screenshot_waits_for_the_first_frames() {
        let p = plan(&opts(&["--sim", "--screenshot", "shot.png"]));
        let lines: Vec<String> = p.steps.iter().map(Step::display).collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[5], format!("sleep {SCREENSHOT_DELAY}"));
        assert_eq!(lines[6], "xcrun simctl io 'iPad Air 13-inch (M3)' screenshot shot.png");
    }

    #[test]
    fn desktop_viewer_is_opened_only_when_requested() {
        let p = plan(&opts(&["--sim", "--show-simulator"]));
        assert_eq!(p.steps.iter().filter(|step| step.program == "open").map(Step::display).collect::<Vec<_>>(), ["open -a Simulator"]);
    }

    #[test]
    fn sim_name_override() {
        let p = plan(&opts(&["--sim", "--sim-name", "iPad Pro"]));
        let last = p.steps.last().expect("steps");
        assert_eq!(last.args, ["simctl", "launch", "--terminate-running-process", "iPad Pro", BUNDLE_ID]);
    }

    #[test]
    fn release_changes_configuration_and_paths() {
        let p = plan(&opts(&["--sim", "--release"]));
        assert!(p.steps[0].display().contains("-configuration Release"));
        assert!(p.steps.iter().any(|s| s.display().contains("Release-iphonesimulator/SolidCraft.app")));
        let p = plan(&opts(&["--device", "--release", "--device-id", "D"]));
        assert!(p.steps.iter().any(|s| s.display().contains("Release-iphoneos/SolidCraft.app")));
    }

    #[test]
    fn device_plan_with_id() {
        let p = plan(&opts(&["--device", "--device-id", "00008103-ABC", "--team", "TEAM123"]));
        let lines: Vec<String> = p.steps.iter().map(Step::display).collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("-destination id=00008103-ABC "));
        assert!(lines[0].contains("-allowProvisioningDeviceRegistration"));
        assert!(lines[0].contains("-allowProvisioningUpdates"));
        assert!(lines[0].contains("DEVELOPMENT_TEAM=TEAM123"));
        assert!(lines[0].ends_with(" build"));
        assert_eq!(lines[1], "xcrun devicectl device install app --device 00008103-ABC target/ios-derived/Build/Products/Debug-iphoneos/SolidCraft.app");
        assert_eq!(lines[2], format!("xcrun devicectl device process launch --device 00008103-ABC {BUNDLE_ID}"));
        assert!(p.notes.is_empty());
        assert!(p.steps.iter().all(|s| !s.allow_failure));
    }

    #[test]
    fn bundle_id_override_reaches_build_and_launch() {
        let p = plan(&opts(&["--device", "--device-id", "D", "--bundle-id", "dev.example.app"]));
        let lines: Vec<String> = p.steps.iter().map(Step::display).collect();
        assert!(lines[0].contains("PRODUCT_BUNDLE_IDENTIFIER=dev.example.app build"));
        assert!(lines[2].ends_with(" dev.example.app"));
        let p = plan(&opts(&["--sim", "--bundle-id", "dev.example.app"]));
        assert!(p.steps[0].display().contains("PRODUCT_BUNDLE_IDENTIFIER=dev.example.app"));
        assert_eq!(p.steps.last().expect("launch").args.last().map(String::as_str), Some("dev.example.app"));
        let p = plan(&opts(&["--sim"]));
        assert!(!p.steps[0].display().contains("PRODUCT_BUNDLE_IDENTIFIER"));
    }

    #[test]
    fn device_without_id_keeps_generic_destination() {
        let p = plan(&opts(&["--device"]));
        let b = p.steps[0].display();
        assert!(b.contains("-destination generic/platform=iOS "));
        assert!(!b.contains("DeviceRegistration"));
    }

    #[test]
    fn device_plan_without_id_lists_devices() {
        let p = plan(&opts(&["--device"]));
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[1].display(), "xcrun devicectl list devices");
        assert_eq!(p.notes.len(), 1);
        assert!(render(&p).contains("note: no device id"));
    }

    #[test]
    fn display_quotes_special_arguments() {
        let s = Step { program: "x".into(), args: vec!["a b".into(), "it's".into(), "".into()], env: vec![("K".into(), "v w".into())], allow_failure: false };
        assert_eq!(s.display(), "K='v w' x 'a b' 'it'\\''s' ''");
    }

    #[test]
    fn non_macos_refused() {
        assert!(host_supported("macos"));
        for os in ["linux", "windows"] {
            assert!(!host_supported(os));
            assert!(refusal(os).contains(os) && refusal(os).contains("macOS"));
        }
    }

    #[test]
    fn help_and_dry_run_work_everywhere() {
        assert!(run(Path::new("."), &["--help"]).is_ok());
        assert!(run(Path::new("."), &["--sim", "--dry-run"]).is_ok());
        assert!(run(Path::new("."), &["--device", "--dry-run"]).is_ok());
        assert!(run(Path::new("."), &["--sim", "--device"]).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn run_refuses_on_this_host() {
        let e = run(Path::new("."), &["--sim"]).unwrap_err();
        assert!(e.contains("needs macOS"));
    }
}
