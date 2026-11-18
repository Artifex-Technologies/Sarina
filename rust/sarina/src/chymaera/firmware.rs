//! Go: `chymaera/firmware`: read-only advice about the Argus firmware fork (Flipper Zero; once called Chymaera Zero):
//! a device's `device_info` checked, and what a merge from upstream would touch. Nothing here writes to a device;
//! flashing stays a person's. Merge risk is decided by whether a path exists upstream, not by its folder.

use crate::gotext::{is_space, trim_space};
use crate::pyfmt::repr_string;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

/// Go's `DeviceInfoExpectations`, in check order.
const EXPECTATIONS: &[(&str, &str)] = &[("firmware_origin_fork", "Argus"), ("enclave_valid", "true"), ("radio_alive", "true")];

const RISK_RULES: &[(&str, &str)] = &[
    ("assets/protobuf/", "wire-format"),
    ("applications_user/", "external-app"),
    ("furi/", "core"),
    ("targets/", "core"),
    ("applications/", "core"),
    ("lib/", "core"),
    ("site_scons/", "build"),
    ("scripts/", "build"),
    ("SConstruct", "build"),
    ("fbt", "build"),
    ("documentation/", "docs"),
];

/// Go's `ClassifyPath`'s area (a label only).
fn classify_path(path: &str) -> &'static str {
    for (prefix, area) in RISK_RULES {
        if path.starts_with(prefix) {
            return area;
        }
    }
    if path.ends_with(".md") {
        return "docs";
    }
    "other"
}

fn split_lines(s: &str) -> Vec<String> {
    s.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_string).collect()
}

/// Go's `ParseDeviceInfo`: "key<spaces>: value" lines; anything else (banner text) is skipped.
pub fn parse_device_info(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for line in split_lines(text) {
        let Some(i) = line.find(':') else { continue };
        let key = trim_space(&line[..i]);
        if key.is_empty() || key.contains(' ') {
            continue;
        }
        fields.insert(key.to_string(), trim_space(&line[i + 1..]).to_string());
    }
    fields
}

static ARGUS_VERSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^ARG-([0-9]+\.[0-9]+\.[0-9]+)(?:r([0-9]+)|-(dev))(?:\+([0-9]+))?$").expect("argusVersion"));
static CHYMAERA_VERSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^([0-9]+\.[0-9]+\.[0-9]+)-CHY-(dev|[0-9][0-9.]*)(?:\+([0-9]+))?$").expect("chymaeraVersion"));
static LEGACY_DEV_BUILD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"-[0-9]+-g[0-9a-f]{7,}$").expect("legacyDevBuild"));

/// Go's `DeviceInfoReport`: warnings make the exit status, notes are observations.
pub struct DeviceInfoReport {
    fields: BTreeMap<String, String>,
    warnings: Vec<String>,
    notes: Vec<String>,
}

impl DeviceInfoReport {
    pub fn ok(&self) -> bool {
        self.warnings.is_empty()
    }

    pub fn as_text(&self) -> String {
        let mut lines = vec!["# Argus device (Flipper Zero)".to_string(), String::new()];
        if self.fields.is_empty() {
            lines.push("No device_info fields parsed.".into());
            return lines.join("\n");
        }
        for key in [
            "firmware_origin_fork",
            "firmware_version",
            "firmware_commit",
            "firmware_origin_git",
            "hardware_model",
            "enclave_valid",
            "radio_alive",
        ] {
            if let Some(v) = self.fields.get(key) {
                lines.push(format!("{key}: {v}"));
            }
        }
        lines.push(String::new());
        if self.warnings.is_empty() {
            lines.push("No warnings.".into());
        } else {
            lines.push("## Warnings".into());
            lines.extend(self.warnings.iter().map(|w| format!("- {w}")));
        }
        if !self.notes.is_empty() {
            lines.push(String::new());
            lines.push("## Notes".into());
            lines.extend(self.notes.iter().map(|n| format!("- {n}")));
        }
        lines.join("\n")
    }
}

/// Go's `CheckDeviceInfo`.
pub fn check_device_info(fields: BTreeMap<String, String>, expected_commit: &str) -> DeviceInfoReport {
    let (mut warnings, mut notes) = (Vec::new(), Vec::new());
    for (key, want) in EXPECTATIONS {
        match fields.get(*key) {
            None => warnings.push(format!("{key} missing from device_info")),
            Some(actual) if *key == "firmware_origin_fork" && crate::gotext::equal_fold(actual, "Chymaera") => {
                notes.push("firmware_origin_fork is 'Chymaera': a build from before the firmware was renamed Argus".to_string())
            }
            Some(actual) if crate::gotext::to_lower(actual) != crate::gotext::to_lower(want) => {
                warnings.push(format!("{key} is {}, expected {}", repr_string(actual), repr_string(want)))
            }
            Some(_) => {}
        }
    }
    if let Some(version) = fields.get("firmware_version") {
        if version == "unknown" || version.is_empty() {
            warnings.push("firmware_version is unknown - the build could not describe itself from git at all".into());
        } else if let Some(a) = ARGUS_VERSION.captures(version) {
            let g = |i: usize| a.get(i).map_or("", |m| m.as_str());
            notes.push(format!("tracks upstream release {}", g(1)));
            if g(3) == "dev" {
                notes.push(format!("no ARG-{}r* tag exists yet, so this build is not a release", g(1)));
            } else if !g(4).is_empty() {
                notes.push(format!("development build: {} commit(s) since ARG-{}r{}", g(4), g(1), g(2)));
            }
        } else if let Some(m) = CHYMAERA_VERSION.captures(version) {
            let g = |i: usize| m.get(i).map_or("", |x| x.as_str());
            notes.push(format!("tracks upstream release {} (version format from before the rename to Argus)", g(1)));
            if g(2) == "dev" {
                notes.push("no chy-* tag exists yet, so this build is not a release".into());
            } else if !g(3).is_empty() {
                notes.push(format!("development build: {} commit(s) since chy-{}", g(3), g(2)));
            }
        } else if LEGACY_DEV_BUILD.is_match(version) {
            notes.push(format!("{version} uses the pre-2026-09-12 version format - the device predates the current firmware"));
        } else {
            notes.push(format!("{version} is not a recognised Argus version string"));
        }
    }
    if !expected_commit.is_empty() {
        let actual = fields.get("firmware_commit").cloned().unwrap_or_default();
        if !actual.is_empty() && !(expected_commit.starts_with(&actual) || actual.starts_with(expected_commit)) {
            warnings.push(format!(
                "firmware_commit {} does not match expected {} - the device is not running this tree",
                repr_string(&actual),
                repr_string(expected_commit)
            ));
        }
    }
    DeviceInfoReport { fields, warnings, notes }
}

// ---- upstream merge impact ----

const REBASE_POLICY_BUDGET: usize = 12;
const TIERS: &[(&str, &str)] = &[
    ("high", "changed on both sides - a merge will conflict here"),
    ("medium", "ours, and the file exists upstream - conflicts if upstream moves it"),
    ("low", "does not exist upstream - additive, cannot conflict"),
];

struct FileRisk {
    path: String,
    tier: &'static str,
    area: &'static str,
}

/// Go's `MergeImpactReport`.
pub struct MergeImpactReport {
    upstream_ref: String,
    base: String,
    upstream_changed: usize,
    local_changed: usize,
    risks: Vec<FileRisk>,
}

impl MergeImpactReport {
    pub fn as_text(&self) -> String {
        let count = |t: &str| self.risks.iter().filter(|r| r.tier == t).count();
        let non_additive = count("high") + count("medium");
        let mut lines = vec![
            format!("# Merge impact: {} -> local firmware tree", self.upstream_ref),
            String::new(),
            format!("merge base: {}", self.base),
            format!("upstream changed since base: {} file(s)", self.upstream_changed),
            format!("local changed since base: {} file(s)", self.local_changed),
            format!("would conflict (high): {}", count("high")),
            format!("ours, exists upstream (medium): {}", count("medium")),
            format!("additive, cannot conflict (low): {}", count("low")),
            String::new(),
        ];
        if non_additive > REBASE_POLICY_BUDGET {
            lines.push(format!(
                "## POLICY ALARM: {non_additive} first-party files exist upstream, over the budget of {REBASE_POLICY_BUDGET}"
            ));
            lines.push("The \"add directories, do not edit upstream files\" rebase policy is being violated. That matters more than any single file below.".into());
            lines.push(String::new());
        }
        if self.risks.is_empty() {
            lines.push("Nothing changed locally since the merge base - the merge should apply cleanly.".into());
            return rstrip(&lines.join("\n"));
        }
        for (tier, why) in TIERS {
            let mut items: Vec<&FileRisk> = self.risks.iter().filter(|r| r.tier == *tier).collect();
            if items.is_empty() {
                continue;
            }
            lines.push(format!("## {tier} ({})", items.len()));
            lines.push(why.to_string());
            items.sort_by(|a, b| a.path.cmp(&b.path));
            lines.extend(items.iter().map(|k| format!("- {} [{}]", k.path, k.area)));
            lines.push(String::new());
        }
        if count("high") == 0 {
            lines.push("No file was changed on both sides - the merge should apply cleanly.".into());
        }
        rstrip(&lines.join("\n"))
    }
}

fn rstrip(s: &str) -> String {
    s.trim_end_matches(is_space).to_string()
}

/// Go's `git`: git run in the repository (300 s), its output trimmed; "git ARGS failed: <its error output, or Go's error>".
fn git(repo: &[u8], args: &[&str]) -> Result<String, String> {
    let failed = |detail: String| format!("git {} failed: {detail}", args.join(" "));
    // Go's StartProcess names a folder it cannot find before it starts anything
    if let Err(e) = std::fs::metadata(crate::gopath::os_path(repo)) {
        return Err(failed(crate::goerr::path("chdir", repo, &e)));
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut sink = |stream: &'static str, b: &[u8]| {
        if stream == "stdout" {
            out.extend_from_slice(b);
        } else {
            err.extend_from_slice(b);
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let env = crate::tools::exec::parent_env();
    let dir = crate::gotext::lossy(repo);
    let end = crate::tools::exec::run_group(
        "git",
        &argv,
        &env,
        Some(&dir),
        deadline,
        &crate::goctx::Ctx::background(),
        crate::tools::exec::GroupKill::Plain,
        &mut sink,
    )
    .map_err(failed)?;
    let status = end.status.map(crate::helpers::exit_text).unwrap_or_else(|| "signal: killed".into());
    if !status.is_empty() {
        let detail = trim_space(&crate::gotext::lossy(&err)).to_string();
        return Err(failed(if detail.is_empty() { status } else { detail }));
    }
    Ok(trim_space(&crate::gotext::lossy(&out)).to_string())
}

fn non_empty_lines(s: &str) -> Vec<String> {
    split_lines(s).iter().map(|l| trim_space(l).to_string()).filter(|l| !l.is_empty()).collect()
}

/// Go's `MergeImpact`: what merging `upstream_ref` into HEAD would touch, without merging.
pub fn merge_impact(repo: &str, upstream_ref: &str) -> Result<MergeImpactReport, String> {
    let upstream_ref = if upstream_ref.is_empty() { "upstream/dev" } else { upstream_ref };
    let head = "HEAD";
    let mut abs = crate::gopath::abs(repo.as_bytes()).unwrap_or_default();
    if let Ok(r) = crate::gopath::eval_symlinks(&abs) {
        abs = r;
    }
    let base = git(&abs, &["merge-base", head, upstream_ref])?;
    let up_range = format!("{base}..{upstream_ref}");
    let upstream_changed = non_empty_lines(&git(&abs, &["diff", "--name-only", &up_range])?);
    let local_range = format!("{base}..{head}");
    let mut local_changed = non_empty_lines(&git(&abs, &["diff", "--name-only", &local_range])?);
    let tree = git(&abs, &["ls-tree", "-r", "--name-only", upstream_ref])?;
    let upstream_files: BTreeSet<String> = non_empty_lines(&tree).into_iter().collect();
    let moved: BTreeSet<&String> = upstream_changed.iter().collect();
    let n_local = local_changed.len();
    local_changed.sort();
    let risks = local_changed
        .into_iter()
        .map(|p| {
            let (exists, changed) = (upstream_files.contains(&p), moved.contains(&p));
            let tier = if !exists {
                "low"
            } else if changed {
                "high"
            } else {
                "medium"
            };
            let area = classify_path(&p);
            FileRisk { path: p, tier, area }
        })
        .collect();
    Ok(MergeImpactReport {
        upstream_ref: upstream_ref.to_string(),
        base,
        upstream_changed: upstream_changed.len(),
        local_changed: n_local,
        risks,
    })
}
