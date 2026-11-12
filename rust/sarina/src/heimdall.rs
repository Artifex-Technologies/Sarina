//! Go: `heimdall` (`grants.go`, the grant gate; the review client comes with step 3). Sarina may ask for more than she
//! has and never give it to herself: the owner answers with `sudo heimdalld grant ...`, which writes
//! /etc/heimdall/grants.json, a file only root can write. The gate is on while the file exists; a file that is a link, not
//! root's, writable by others, in a folder others can write, or that does not read, turns it on with nothing allowed.

use crate::goerr;
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotime::Time;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// Heimdall's decisions file.
pub const DEFAULT_GRANTS_PATH: &str = "/etc/heimdall/grants.json";

/// One hand-off the owner approved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Escalation {
    pub tier: String,
    pub session_id: String,
    pub until: Time,
}

/// What the owner decided, as far as Sarina needs it (Go's `Grants`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grants {
    /// the file exists: the gate decides
    pub on: bool,
    access_tier: Option<i64>,
    escalations: Vec<Escalation>,
    /// why the file was not trusted (then the gate is on and allows nothing)
    pub problem: String,
}

static ESCALATION: Ty = Ty::Struct(
    "heimdall.grantEscalation",
    "grantEscalation",
    &[Field("tier", &Ty::Str), Field("session_id", &Ty::Str), Field("until", &Ty::Time)],
);
static GRANTS: Ty = Ty::Struct(
    r#"struct { AccessTier *int "json:\"access_tier\""; Escalations []heimdall.grantEscalation "json:\"escalations\"" }"#,
    "",
    &[Field("access_tier", &Ty::Ptr(&Ty::Int)), Field("escalations", &Ty::Slice(&ESCALATION))],
);

fn untrusted(md: &std::fs::Metadata, owner: u32) -> &'static str {
    if md.file_type().is_symlink() {
        return "is a link";
    }
    if md.uid() != owner {
        return "is not root's";
    }
    if md.permissions().mode() & 0o022 != 0 {
        return "can be written by others";
    }
    ""
}

/// Go's `os.ReadFile` with its error texts ("open ...", "read ...").
fn read_file(path: &[u8]) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(gopath::os_path(path)).map_err(|e| goerr::path("open", path, &e))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| goerr::path("read", path, &e))?;
    Ok(b)
}

/// Go's `LoadGrants` for the file at `path`, which `owner` (root: 0) must own, as must its folder.
pub fn load_grants_at(path: &[u8], owner: u32) -> Grants {
    let md = match std::fs::symlink_metadata(gopath::os_path(path)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Grants::default(),
        Err(e) => return Grants { on: true, problem: goerr::path("lstat", path, &e), ..Grants::default() },
        Ok(md) => md,
    };
    let mut g = Grants { on: true, ..Grants::default() };
    let why = untrusted(&md, owner);
    if !why.is_empty() {
        g.problem = format!("{} {why}", String::from_utf8_lossy(path));
        return g;
    }
    let dir = gopath::dir(path);
    let dir_ok = std::fs::metadata(gopath::os_path(&dir)).is_ok_and(|d| untrusted(&d, owner).is_empty());
    if !dir_ok {
        g.problem = format!("{} is not root's alone", String::from_utf8_lossy(&dir));
        return g;
    }
    let b = match read_file(path) {
        Ok(b) => b,
        Err(e) => {
            g.problem = e;
            return g;
        }
    };
    grants_from_json(&b)
}

/// A gate that is on with the grants file's content (Go's `json.Unmarshal` into its struct): on an error only the problem
/// is kept.
pub fn grants_from_json(b: &[u8]) -> Grants {
    let mut g = Grants { on: true, ..Grants::default() };
    let (v, err) = gostruct::unmarshal(&GRANTS, b);
    if let Some(e) = err {
        g.problem = e.0;
        return g;
    }
    if let Some(Val::Ptr(Some(t))) = gostruct::field(&GRANTS, &v, "access_tier") {
        if let Val::Int(n) = **t {
            g.access_tier = Some(n);
        }
    }
    if let Some(Val::Slice(Some(items))) = gostruct::field(&GRANTS, &v, "escalations") {
        for it in items {
            let until = match gostruct::field(&ESCALATION, it, "until") {
                Some(Val::Time(t)) => *t,
                _ => Time::default(),
            };
            g.escalations.push(Escalation {
                tier: gostruct::str_field(&ESCALATION, it, "tier").to_string(),
                session_id: gostruct::str_field(&ESCALATION, it, "session_id").to_string(),
                until,
            });
        }
    }
    g
}

/// Go's `LoadGrants`: Heimdall's file, root's.
pub fn load_grants() -> Grants {
    load_grants_at(DEFAULT_GRANTS_PATH.as_bytes(), 0)
}

impl Grants {
    /// The access tier Sarina may use: her own setting while the gate is off, else the lower of it and the owner's cap
    /// (no cap in the file: 0, read-only).
    pub fn tier_cap(&self, own: i64) -> i64 {
        if !self.on {
            return own;
        }
        match self.access_tier {
            None => 0,
            Some(cap) => own.min(cap.max(0)),
        }
    }

    /// Whether the owner approved a hand-off to `tier` (for this session, or for any) that has not expired.
    pub fn escalation_allowed(&self, tier: &str, session_id: &str, now: &Time) -> bool {
        self.escalations.iter().any(|e| e.tier == tier && (e.session_id.is_empty() || e.session_id == session_id) && now.before(&e.until))
    }
}
