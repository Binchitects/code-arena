//! A newer Code Arena offered by Arena: said, fetched, checked and put in place, as the old Code Arena's updates.
//!
//! Arena serves the builds of its own version (`ca_arena::downloads`), so a newer one there is the one to run. When
//! the driver starts, and once a day after, it asks (within a few seconds, at most once a day: when it last asked
//! is kept in the data folder's `update-check.json`) unless `update.check` is off, and says what it found:
//! `update_checked{available, version, current, said, asked}`; `state.update` says it too. `update_check` asks
//! again at once (`asked: true` in the reply).
//!
//! `update_install` downloads the build for this system beside the program (`code-arena.new`), saying
//! `update_progress{version, pct}` as it comes; checks its size and SHA-256 as Arena lists them; and, when Arena
//! signs its builds, that its code-signing certificate comes from the Arena's signing CA and signed the list of
//! builds that holds this one (see [`signature_problem`]). Then it is put in the program's place (on Windows the
//! running one moves aside first, as `code-arena.exe.old`), and `update_done{version, program, said}` says it is
//! there to restart into; `update_done{error}` says why nothing changed.
//!
//! The signing CA is the one the installer kept beside the program, `code-arena-signing-ca.pem` (PEM); without
//! it, the CA this machine trusts for Arena (`ARENA_CA_CERT`, the connection's); without that, the system's.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, UnixTime};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ide_ext::{Area, Rule, Shared, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("update_check", Rule::Turn), ("update_install", Rule::Turn)];

    fn start(sh: &Shared) {
        let me = sh.kept.of::<Updates>();
        me.keep(sh);
        if me.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = me.clone();
        tokio::spawn(async move {
            loop {
                if let Some(sh) = me.latest()
                    && sh.settings.bool(&ca_settings::schema::UPDATE_CHECK)
                {
                    check(&me, &sh, false).await;
                }
                tokio::time::sleep(EVERY).await;
            }
        });
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let me = sh.kept.of::<Updates>();
        me.keep(sh);
        match crate::ide_ext::kind(m) {
            "update_check" => {
                let sh = sh.clone();
                tokio::spawn(async move { check(&me, &sh, true).await });
            }
            "update_install" => {
                if me.busy.swap(true, Ordering::SeqCst) {
                    return true;
                }
                let sh = sh.clone();
                tokio::spawn(async move {
                    install_now(&me, &sh).await;
                    me.busy.store(false, Ordering::SeqCst);
                });
            }
            _ => return false,
        }
        true
    }

    fn state(sh: &Shared, _p: Option<&crate::session::Prepared>, v: &mut Value) {
        let me = sh.kept.of::<Updates>();
        me.keep(sh);
        let found = me.inner.lock().map(|i| (i.found.clone(), i.installed.clone())).unwrap_or_default();
        v["update"] = json!({
            "available": found.0.is_some(),
            "version": found.0,
            "current": ca_core::VERSION,
            "said": found.0.as_deref().map(|f| said(f, ca_core::VERSION)),
            "installed": found.1,
        });
    }
}

/// How often the driver asks, and how long it waits for Arena.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const WAIT: Duration = Duration::from_secs(5);

/// The Arena's signing CA as the installers keep it beside the program.
pub const SIGNING_CA_FILE: &str = "code-arena-signing-ca.pem";

/// What the area keeps for as long as the driver runs.
#[derive(Default)]
struct Updates {
    /// The daily look is begun (once, whatever sessions follow).
    started: AtomicBool,
    /// An update is being put in place.
    busy: AtomicBool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The session as it last stood: its Arena, its settings, its data folder.
    sh: Option<Shared>,
    /// The program as it was when the driver started (on Linux the running one's path says "(deleted)" once it
    /// is replaced).
    exe: Option<PathBuf>,
    /// The newer version found.
    found: Option<String>,
    /// The version put in place, to restart into.
    installed: Option<String>,
}

impl Updates {
    fn keep(&self, sh: &Shared) {
        if let Ok(mut i) = self.inner.lock() {
            if i.exe.is_none() {
                i.exe = std::env::current_exe().ok();
            }
            i.sh = Some(sh.clone());
        }
    }

    fn latest(&self) -> Option<Shared> {
        self.inner.lock().ok().and_then(|i| i.sh.clone())
    }

    fn exe(&self) -> Option<PathBuf> {
        self.inner.lock().ok().and_then(|i| i.exe.clone())
    }

    fn found(&self, version: Option<String>) {
        if let Ok(mut i) = self.inner.lock() {
            i.found = version;
        }
    }
}

/// This program's system, as Arena names its builds (the release's names: linux-x64, linux-x64-static, osx-arm64,
/// win-x64…); a build without the native window is the static one.
pub fn rid() -> String {
    let os = match std::env::consts::OS {
        "macos" => "osx",
        "windows" => "win",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let static_build = os == "linux" && !cfg!(feature = "native");
    format!("{os}-{arch}{}", if static_build { "-static" } else { "" })
}

/// Whether version `a` is newer than `b` (x.y.z; a pre-release comes before its release).
pub fn newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> Option<(Vec<u64>, String)> {
        let v = v.trim().trim_start_matches('v');
        let (number, pre) = v.split_once('-').unwrap_or((v, ""));
        let n: Option<Vec<u64>> = number.split('.').map(|p| p.parse().ok()).collect();
        let mut n = n?;
        n.resize(4, 0);
        Some((n, pre.to_string()))
    }
    let (Some((x, xp)), Some((y, yp))) = (parse(a), parse(b)) else { return false };
    match x.cmp(&y) {
        std::cmp::Ordering::Equal => match (xp.is_empty(), yp.is_empty()) {
            (true, false) => true,
            (false, false) => xp > yp,
            _ => false,
        },
        c => c == std::cmp::Ordering::Greater,
    }
}

/// What a newer build means for the person, in one line.
pub fn said(version: &str, current: &str) -> String {
    format!("Code Arena {version} is available from Arena (this is {current}).")
}

/// Asks Arena (or, unless `asked`, reads what it said within the day) and says what it found.
async fn check(me: &Updates, sh: &Shared, asked: bool) {
    let current = ca_core::VERSION;
    let file = sh.paths.data.join("update-check.json");
    let mut error = None;
    let version = match recent(&file, chrono::Utc::now()).filter(|_| !asked) {
        Some(v) => v,
        None => match &sh.arena {
            None => {
                error = Some("Not signed in to an Arena yet: sign in, then check again.".to_string());
                None
            }
            Some(arena) => {
                let wait = if asked { Duration::from_secs(30) } else { WAIT };
                match tokio::time::timeout(wait, arena.builds()).await {
                    Ok(Ok(b)) => {
                        let v = (newer(&b.version, current) && b.of(&rid()).is_some()).then_some(b.version);
                        let kept = json!({"checked": chrono::Utc::now().to_rfc3339(), "version": v});
                        let _ = std::fs::create_dir_all(&sh.paths.data);
                        let _ = std::fs::write(&file, kept.to_string());
                        v
                    }
                    Ok(Err(e)) => {
                        error = Some(format!("Arena could not be asked for a newer Code Arena: {e}"));
                        None
                    }
                    Err(_) => {
                        error = Some("Arena did not answer in time: try again in a moment.".to_string());
                        None
                    }
                }
            }
        },
    };
    let version = version.filter(|v| newer(v, current));
    me.found(version.clone());
    if let Some(v) = &version {
        crate::log::line(&said(v, current));
    }
    let mut reply = json!({
        "type": "update_checked",
        "available": version.is_some(),
        "version": version,
        "current": current,
        "said": version.as_deref().map(|v| said(v, current)),
        "asked": asked,
    });
    if let Some(e) = error.filter(|_| asked) {
        reply["error"] = json!(e);
    }
    send(&sh.out, reply);
}

/// What Arena said within the day (`Some(None)`: nothing newer), from the file it is kept in.
fn recent(file: &Path, now: chrono::DateTime<chrono::Utc>) -> Option<Option<String>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let at = chrono::DateTime::parse_from_rfc3339(v["checked"].as_str()?).ok()?;
    let age = now.signed_duration_since(at);
    (age >= chrono::TimeDelta::zero() && age < chrono::TimeDelta::days(1))
        .then(|| v["version"].as_str().map(str::to_string))
}

/// The update put in place and said, or why not.
async fn install_now(me: &Updates, sh: &Shared) {
    let out = sh.out.clone();
    let Some(exe) = me.exe() else {
        send(&out, json!({"type": "update_done", "error": "This program's own path is not known: nothing changed."}));
        return;
    };
    let Some(arena) = &sh.arena else {
        send(&out, json!({"type": "update_done", "error": "Not signed in to an Arena yet: sign in, then update."}));
        return;
    };
    let pinned = exe.parent().map(|d| d.join(SIGNING_CA_FILE)).filter(|f| f.is_file());
    let trust = match pinned.or_else(|| sh.ca.clone()) {
        Some(f) => match std::fs::read(&f) {
            Ok(pem) => Trust::Pem(pem),
            Err(e) => {
                let error = format!("The signing CA {} could not be read ({e}): nothing changed.", f.display());
                send(&out, json!({"type": "update_done", "error": error}));
                return;
            }
        },
        None => Trust::System,
    };
    let mut last = None;
    let progress = |version: &str, pct: u8| {
        if last != Some(pct) {
            last = Some(pct);
            send(&out, json!({"type": "update_progress", "version": version, "pct": pct}));
        }
    };
    crate::log::line(&format!("Updating Code Arena {} from {}", ca_core::VERSION, arena.base()));
    match install(arena, &exe, ca_core::VERSION, &rid(), &trust, UnixTime::now(), progress).await {
        Ok(done) => {
            crate::log::line(&done.said);
            if let Ok(mut i) = me.inner.lock() {
                i.installed = Some(done.version.clone());
            }
            send(
                &out,
                json!({"type": "update_done", "version": done.version, "program": exe.display().to_string(),
                       "said": done.said}),
            );
        }
        Err(error) => {
            crate::log::line(&error);
            send(&out, json!({"type": "update_done", "error": error}));
        }
    }
}

/// Whom a build's signature is checked against.
pub enum Trust {
    /// The CAs in this PEM.
    Pem(Vec<u8>),
    /// The system's.
    System,
}

/// An update put in place.
#[derive(Debug)]
pub struct Installed {
    pub version: String,
    pub said: String,
}

/// Arena's newer build put in `exe`'s place: downloaded beside it, checked, made runnable, renamed over it. What
/// it did, or why it did not (nothing is changed then).
pub async fn install(
    arena: &ca_arena::ArenaClient,
    exe: &Path,
    current: &str,
    rid: &str,
    trust: &Trust,
    now: UnixTime,
    mut progress: impl FnMut(&str, u8),
) -> Result<Installed, String> {
    let name = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name != "code-arena" || crate::log::built_here(exe) {
        return Err("This Code Arena runs from its build folder: build it again, or download it from Arena's \
                    Connect your tools."
            .into());
    }
    let builds = match tokio::time::timeout(Duration::from_secs(30), arena.builds()).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return Err(format!("Arena could not be asked for a newer Code Arena: {e}")),
        Err(_) => return Err("Arena did not answer in time: try again in a moment.".into()),
    };
    let Some(build) = builds.of(rid).filter(|_| newer(&builds.version, current)) else {
        return Err(format!("Code Arena {current} is the newest this Arena has for {rid}."));
    };
    let version = builds.version.clone();
    let temporary = with_suffix(exe, ".new");
    let fetched = fetch(arena, rid, build.size, &temporary, |pct| progress(&version, pct)).await;
    let checked = fetched.and_then(|(length, sum)| {
        let want = build.sha256.as_deref().unwrap_or("");
        if (build.size > 0 && length != build.size) || (!want.is_empty() && !want.eq_ignore_ascii_case(&sum)) {
            return Err(format!(
                "The download did not match what Arena lists ({length} bytes, SHA-256 {}…): nothing changed. Try again.",
                &sum[..12]
            ));
        }
        Ok(sum)
    });
    let sum = match checked {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
    };
    if builds.signed() {
        let file = exe.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let problem = match signed_files(arena).await {
            Ok((sums, sig, pem)) => signature_problem(&sums, &sig, &pem, trust, &format!("{rid}/{file}"), &sum, now),
            Err(e) => Some(format!("Arena did not give the signature: {e}")),
        };
        if let Some(p) = problem {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("The download is not the one Arena's CA signed ({p}): nothing changed."));
        }
    }
    if let Err(e) = swap(&temporary, exe) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Code Arena could not be replaced ({e}): nothing changed."));
    }
    let by = if builds.signed() { ", signed by Arena's CA" } else { "" };
    Ok(Installed { said: format!("Code Arena {current} → {version}{by}: restart to run it."), version })
}

/// `path` with `suffix` after its name (`code-arena.new`, `code-arena.exe.old`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The build downloaded to `to`: its length and SHA-256, counted as it comes.
async fn fetch(
    arena: &ca_arena::ArenaClient,
    name: &str,
    size: u64,
    to: &Path,
    mut progress: impl FnMut(u8),
) -> Result<(u64, String), String> {
    let mut file = std::fs::File::create(to).map_err(|e| format!("The download could not be written ({e}): nothing changed."))?;
    let (mut hash, mut length, mut failed) = (Sha256::new(), 0u64, None);
    progress(0);
    let whole = arena
        .download(name, |piece, said| {
            if let Err(e) = file.write_all(piece) {
                failed = Some(e);
                return false;
            }
            hash.update(piece);
            length += piece.len() as u64;
            let of = if size > 0 { size } else { said.unwrap_or(0) };
            if of > 0 {
                progress((length.saturating_mul(100) / of).min(100) as u8);
            }
            true
        })
        .await;
    match (whole, failed) {
        (_, Some(e)) => Err(format!("The download could not be written ({e}): nothing changed.")),
        (Err(e), _) => Err(format!("Arena did not give the build ({e}): try again in a moment.")),
        (Ok(_), None) => {
            file.flush().map_err(|e| format!("The download could not be written ({e}): nothing changed."))?;
            Ok((length, hash.finalize().iter().map(|b| format!("{b:02x}")).collect()))
        }
    }
}

/// The builds' list, its signature and the certificate that made it.
async fn signed_files(arena: &ca_arena::ArenaClient) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), ca_arena::ArenaError> {
    Ok((
        arena.download_bytes("SHA256SUMS").await?,
        arena.download_bytes("SHA256SUMS.sig").await?,
        arena.download_bytes("code-signing.crt").await?,
    ))
}

/// The checked download put in the program's place: made runnable and renamed over it, or (on Windows) the
/// running one moved aside first.
fn swap(temporary: &Path, exe: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(0o755))?;
    }
    if cfg!(windows) {
        let aside = with_suffix(exe, ".old");
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(exe, &aside)?;
        if let Err(e) = std::fs::rename(temporary, exe) {
            let _ = std::fs::rename(&aside, exe);
            return Err(e);
        }
        return Ok(());
    }
    std::fs::rename(temporary, exe)
}

/// id-kp-codeSigning, 1.3.6.1.5.5.7.3.3, as DER writes it.
const CODE_SIGNING: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];

/// Why a build is not the one Arena's CA signed, or None when it is: the certificate (the first in `pem`, the rest
/// its chain) is one for code signing from a CA `trust` holds; the builds' list (`sums`) is signed with its key
/// (RSA PKCS#1 v1.5, SHA-256: `signature`); and this build's SHA-256 is in the list as `sha256sum` writes it
/// (`{sum}  {entry}`).
pub fn signature_problem(
    sums: &[u8],
    signature: &[u8],
    pem: &[u8],
    trust: &Trust,
    entry: &str,
    sum: &str,
    now: UnixTime,
) -> Option<String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem).filter_map(Result::ok).collect();
    let Some((leaf, chain)) = certs.split_first() else { return Some("no certificate came with it".into()) };
    let roots: Vec<CertificateDer<'static>> = match trust {
        Trust::Pem(p) => CertificateDer::pem_slice_iter(p).filter_map(Result::ok).collect(),
        Trust::System => rustls_native_certs::load_native_certs().certs,
    };
    let anchors: Vec<_> = roots.iter().filter_map(|c| webpki::anchor_from_trusted_cert(c).ok()).collect();
    if anchors.is_empty() {
        return Some("there is no CA to check it with".into());
    }
    let Ok(cert) = webpki::EndEntityCert::try_from(leaf) else {
        return Some("its certificate could not be read".into());
    };
    let algs = webpki::ALL_VERIFICATION_ALGS;
    let usage = webpki::KeyUsage::required(CODE_SIGNING);
    if let Err(e) = cert.verify_for_usage(algs, &anchors, chain, now, usage, None, None) {
        return Some(format!("its certificate is not one for code signing from the Arena's signing CA: {e}"));
    }
    if cert.verify_signature(webpki::ring::RSA_PKCS1_2048_8192_SHA256, sums, signature).is_err() {
        return Some("the builds' list is not the one its certificate signed".into());
    }
    let line = format!("{sum}  {entry}");
    let listed = String::from_utf8_lossy(sums).lines().any(|l| l.trim_end_matches('\r') == line);
    (!listed).then(|| format!("{entry} is not in the list signed"))
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const CA: &[u8] = include_bytes!("../tests/fixtures/update/ca.pem");
    const OTHER_CA: &[u8] = include_bytes!("../tests/fixtures/update/other-ca.pem");
    const CERT: &[u8] = include_bytes!("../tests/fixtures/update/code-signing.crt");
    const SUMS: &[u8] = include_bytes!("../tests/fixtures/update/SHA256SUMS");
    const SIG: &[u8] = include_bytes!("../tests/fixtures/update/SHA256SUMS.sig");
    const BUILD: &[u8] = include_bytes!("../tests/fixtures/update/build");

    fn sum_of(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn now() -> UnixTime {
        UnixTime::since_unix_epoch(Duration::from_secs(1_800_000_000))
    }

    #[test]
    fn versions_are_compared_as_numbers_with_a_pre_release_first() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("v1.2.3", "1.2.2"));
        assert!(newer("1.2.3", "1.2.3-beta"));
        assert!(newer("1.2.3-rc2", "1.2.3-rc1"));
        assert!(!newer("1.2.3", "1.2.3"));
        assert!(!newer("1.2.3-beta", "1.2.3"));
        assert!(!newer("garbage", "1.0.0"));
        assert_eq!(said("9.1.0", "9.0.0"), "Code Arena 9.1.0 is available from Arena (this is 9.0.0).");
    }

    #[test]
    fn the_system_is_named_as_the_builds_are() {
        let r = rid();
        assert!(["linux-", "osx-", "win-"].iter().any(|p| r.starts_with(p)) || r.contains('-'), "{r}");
    }

    #[test]
    fn what_arena_said_within_the_day_is_kept() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("update-check.json");
        let now = chrono::Utc::now();
        assert_eq!(recent(&file, now), None);
        std::fs::write(&file, json!({"checked": now.to_rfc3339(), "version": "9.9.9"}).to_string()).expect("write");
        assert_eq!(recent(&file, now), Some(Some("9.9.9".into())));
        assert_eq!(recent(&file, now + chrono::TimeDelta::days(2)), None);
    }

    #[test]
    fn a_build_signed_by_the_arena_ca_is_taken() {
        let sum = sum_of(BUILD);
        let trust = Trust::Pem(CA.to_vec());
        assert_eq!(signature_problem(SUMS, SIG, CERT, &trust, "linux-x64/code-arena", &sum, now()), None);
        // Another build's sum, a list changed after it was signed, a signature that is not its, another CA.
        let p = signature_problem(SUMS, SIG, CERT, &trust, "linux-x64/code-arena", &sum_of(b"other"), now());
        assert!(p.is_some_and(|p| p.contains("not in the list")));
        let mut changed = SUMS.to_vec();
        changed[0] ^= 1;
        let p = signature_problem(&changed, SIG, CERT, &trust, "linux-x64/code-arena", &sum, now());
        assert!(p.is_some_and(|p| p.contains("not the one its certificate signed")));
        let mut bad = SIG.to_vec();
        bad[10] ^= 1;
        assert!(signature_problem(SUMS, &bad, CERT, &trust, "linux-x64/code-arena", &sum, now()).is_some());
        let other = Trust::Pem(OTHER_CA.to_vec());
        let p = signature_problem(SUMS, SIG, CERT, &other, "linux-x64/code-arena", &sum, now());
        assert!(p.is_some_and(|p| p.contains("signing CA")));
        assert!(signature_problem(SUMS, SIG, b"", &trust, "linux-x64/code-arena", &sum, now()).is_some());
        // The CA's own certificate is not one for code signing.
        assert!(signature_problem(SUMS, SIG, CA, &trust, "linux-x64/code-arena", &sum, now()).is_some());
    }

    /// A fake Arena serving a newer build, signed or not.
    async fn arena(sig: &[u8], signed: bool) -> MockServer {
        let server = MockServer::start().await;
        let mut builds = json!({"version": "99.0.0", "builds": [{"rid": "linux-x64", "size": BUILD.len(), "sha256": sum_of(BUILD)}]});
        if signed {
            builds["signature"] = json!({});
        }
        Mock::given(method("GET")).and(path(ca_arena::downloads::ROOT)).respond_with(ResponseTemplate::new(200).set_body_json(builds)).mount(&server).await;
        for (name, body) in [("linux-x64", BUILD), ("SHA256SUMS", SUMS), ("SHA256SUMS.sig", sig), ("code-signing.crt", CERT)] {
            Mock::given(method("GET"))
                .and(path(format!("{}/{name}", ca_arena::downloads::ROOT)))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
                .mount(&server)
                .await;
        }
        server
    }

    #[tokio::test]
    async fn a_signed_build_takes_the_programs_place() {
        let server = arena(SIG, true).await;
        let client = ca_arena::ArenaClient::new(&server.uri(), None, None).expect("client");
        let dir = tempfile::tempdir().expect("dir");
        let exe = dir.path().join("code-arena");
        std::fs::write(&exe, "old program").expect("write");
        let mut seen = Vec::new();
        let done = install(&client, &exe, "1.0.0", "linux-x64", &Trust::Pem(CA.to_vec()), now(), |_, p| seen.push(p))
            .await
            .expect("installed");
        assert_eq!(done.version, "99.0.0");
        assert_eq!(done.said, "Code Arena 1.0.0 → 99.0.0, signed by Arena's CA: restart to run it.");
        assert_eq!(std::fs::read(&exe).expect("read"), BUILD);
        assert_eq!((seen.first(), seen.last()), (Some(&0), Some(&100)));
        assert!(!dir.path().join("code-arena.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&exe).expect("meta").permissions().mode() & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn a_bad_signature_changes_nothing() {
        let mut bad = SIG.to_vec();
        bad[0] ^= 1;
        let server = arena(&bad, true).await;
        let client = ca_arena::ArenaClient::new(&server.uri(), None, None).expect("client");
        let dir = tempfile::tempdir().expect("dir");
        let exe = dir.path().join("code-arena");
        std::fs::write(&exe, "old program").expect("write");
        let e = install(&client, &exe, "1.0.0", "linux-x64", &Trust::Pem(CA.to_vec()), now(), |_, _| {})
            .await
            .expect_err("refused");
        assert!(e.starts_with("The download is not the one Arena's CA signed"), "{e}");
        assert_eq!(std::fs::read_to_string(&exe).expect("read"), "old program");
        assert!(!dir.path().join("code-arena.new").exists());
    }

    #[tokio::test]
    async fn nothing_newer_or_a_build_folder_changes_nothing() {
        let server = arena(SIG, false).await;
        let client = ca_arena::ArenaClient::new(&server.uri(), None, None).expect("client");
        let dir = tempfile::tempdir().expect("dir");
        let exe = dir.path().join("code-arena");
        std::fs::write(&exe, "old program").expect("write");
        let trust = Trust::Pem(CA.to_vec());
        let e = install(&client, &exe, "99.0.0", "linux-x64", &trust, now(), |_, _| {}).await.expect_err("newest");
        assert_eq!(e, "Code Arena 99.0.0 is the newest this Arena has for linux-x64.");
        let e = install(&client, &exe, "1.0.0", "osx-arm64", &trust, now(), |_, _| {}).await.expect_err("none");
        assert!(e.contains("newest this Arena has for osx-arm64"), "{e}");
        let built = dir.path().join("target").join("debug");
        std::fs::create_dir_all(&built).expect("dir");
        let e = install(&client, &built.join("code-arena"), "1.0.0", "linux-x64", &trust, now(), |_, _| {})
            .await
            .expect_err("build folder");
        assert!(e.contains("build folder"), "{e}");
        // Unsigned builds are taken on their size and SHA-256.
        let done = install(&client, &exe, "1.0.0", "linux-x64", &trust, now(), |_, _| {}).await.expect("installed");
        assert_eq!(done.said, "Code Arena 1.0.0 → 99.0.0: restart to run it.");
    }
}
