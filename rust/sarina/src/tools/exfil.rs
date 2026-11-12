//! Go: `go/tools/exfil.go`. Code that takes keys or passwords off this computer is never written, whatever the request says:
//! content that both reads a credential store and sends data over the network is refused by the write and edit tools, and
//! code kept to run later elsewhere (a forged agent) may not name a credential store or a private folder at all.

use std::sync::LazyLock;

use regex::Regex;

use crate::goregexp;
use crate::gotext::{trim_space, trim_space_bytes};
use crate::tools::private::{home_dir, CREDENTIAL_STORES};

/// The patterns exactly as the Go code writes them, read with Go's meaning ([`goregexp`]).
static CREDENTIAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(concat!(
        r#"(?i)(~|\$HOME|\$\{HOME\}|/home/[^/\s]+|/root)/\.(ssh|gnupg|password-store|aws|kube|docker|git-credentials|netrc|pgpass)\b|"#,
        r#"\.local/share/keyrings|\.mozilla/firefox|\.config/(vivaldi|chromium|google-chrome|BraveSoftware|microsoft-edge)|/etc/(shadow|gshadow|sudoers)\b|"#,
        r#"(^|[\s'"=/])\.(ssh|gnupg|password-store)([/\s'"]|$)|\bid_(rsa|ed25519|ecdsa|dsa)\b|\.git-credentials\b|\bsecring\.gpg\b|\bkey4\.db\b|\blogins\.json\b|Login Data\b"#,
    ))
    .expect("credential pattern")
});

static NETWORK_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(concat!(
        r#"(?i)\b(curl|wget|nc|ncat|netcat|socat|scp|sftp|ftp|tftp|rsync\s+\S+\s+\S+:|telnet|ssh\s+\S+@)\b|/dev/(tcp|udp)/|"#,
        r#"\brequests\.(post|put|patch)\b|\burllib\.request\b|\bhttp\.client\b|\bsocket\.(socket|create_connection)\b|\bsmtplib\b|"#,
        r#"\bfetch\(|\bXMLHttpRequest\b|\bInvoke-(WebRequest|RestMethod)\b|\bnet/http\b|\bhttp\.(Post|NewRequest)\b|\bdiscord(app)?\.com/api/webhooks\b"#,
    ))
    .expect("network pattern")
});

/// Go's `ExfilRisk`: what makes content a credential exfiltration (what it reads, how it sends), or "".
pub fn exfil_risk(content: &str) -> String {
    let Some(cred) = CREDENTIAL_RE.find(content) else { return String::new() };
    let Some(net) = NETWORK_RE.find(content) else { return String::new() };
    format!("it reads {} and sends data over the network ({})", trim_space(cred.as_str()), trim_space(net.as_str()))
}

/// Go's `PrivateReference`: the first credential store or private folder that content names (as ~/x, $HOME/x or the full
/// path), or "". Unlike [`exfil_risk`] it needs no network part: it is for code kept to run later somewhere else.
pub fn private_reference(content: &str) -> String {
    if let Some(m) = CREDENTIAL_RE.find(content) {
        return trim_space(m.as_str()).trim_matches(['\'', '"', '=', '/']).to_owned();
    }
    let home = home_dir();
    let mut dirs: Vec<String> =
        CREDENTIAL_STORES.iter().map(|rel| rel.split('*').next().unwrap_or(rel).trim_end_matches_once('/').to_owned()).collect();
    let private = std::env::var_os("SARINA_PRIVATE_DIRS").unwrap_or_default();
    for d in private.as_encoded_bytes().split(|&c| c == b':') {
        // an entry that is not UTF-8 can never be found in content (which is): skipping it is what Go's comparisons come to
        let Ok(d) = std::str::from_utf8(trim_space_bytes(d)) else { continue };
        if let Some(rest) = d.strip_prefix("~/") {
            dirs.push(rest.to_owned());
        } else if let Some(rest) = (!home.is_empty()).then(|| d.strip_prefix(&format!("{home}/"))).flatten() {
            dirs.push(rest.to_owned());
        } else if d.starts_with('/') && content.contains(d) {
            return d.to_owned();
        }
    }
    for rel in dirs.iter().filter(|r| !r.is_empty()) {
        let mut forms = vec![format!("~/{rel}"), format!("$HOME/{rel}"), format!("${{HOME}}/{rel}")];
        if !home.is_empty() {
            forms.push(format!("{home}/{rel}"));
        }
        if let Some(f) = forms.into_iter().find(|f| content.contains(f.as_str())) {
            return f;
        }
    }
    String::new()
}

/// Go's `strings.TrimSuffix`: one copy of the suffix at most.
trait TrimOnce {
    fn trim_end_matches_once(&self, suffix: char) -> &str;
}

impl TrimOnce for str {
    fn trim_end_matches_once(&self, suffix: char) -> &str {
        self.strip_suffix(suffix).unwrap_or(self)
    }
}
