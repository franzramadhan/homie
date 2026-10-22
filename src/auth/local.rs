//! Built-in username and password login. The password is stored only as an
//! argon2id hash: from the config (`homie hash-password`), or generated at
//! the first start and written to a file in `state_dir`. There is no default
//! password: a public one would open every instance that never changed it.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

use crate::{Result, config};

const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOCKOUT: Duration = Duration::from_secs(5 * 60);

pub struct Local {
    pub username: String,
    hash: String,
    throttle: Mutex<Throttle>,
}

#[derive(Default)]
struct Throttle {
    failures: u32,
    first: Option<Instant>,
    locked_until: Option<Instant>,
}

pub fn hash(password: &str) -> Result<String> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| format!("hashing: {e}"))?
        .to_string())
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)
        .and_then(|mut f| f.write_all(contents.as_bytes()))
        .map_err(|e| format!("writing {}: {e}", path.display()).into())
}

/// The stored hash, or a new random password: its hash is kept in
/// `local-login.hash`, the password itself in `initial-password`, both 0600.
fn bootstrap(state_dir: &Path) -> Result<String> {
    let hash_file = state_dir.join("local-login.hash");
    if let Ok(h) = std::fs::read_to_string(&hash_file) {
        return Ok(h.trim().to_string());
    }
    std::fs::create_dir_all(state_dir)
        .map_err(|e| format!("state_dir {}: {e}", state_dir.display()))?;
    let password: String = openidconnect::CsrfToken::new_random()
        .secret()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(24)
        .collect();
    let h = hash(&password)?;
    let initial: PathBuf = state_dir.join("initial-password");
    write_private(&initial, &format!("{password}\n"))?;
    write_private(&hash_file, &format!("{h}\n"))?;
    tracing::warn!(
        msg = "local login password generated",
        file = %initial.display(),
        note = "read it once, then set local_login.password_hash and delete the file"
    );
    Ok(h)
}

/// `homie hash-password`: reads one line from stdin (without echo on a
/// terminal) and prints the argon2 hash.
pub fn hash_password_cli() -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let tty = std::io::stdin().is_terminal();
    let stty = |arg: &str| {
        let _ = std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .status();
    };
    if tty {
        eprint!("Password: ");
        std::io::stderr().flush()?;
        stty("-echo");
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if tty {
        stty("echo");
        eprintln!();
    }
    read?;
    let password = line.trim_end_matches(['\r', '\n']);
    if password.chars().count() < 12 {
        return Err("use at least 12 characters".into());
    }
    println!("{}", hash(password)?);
    Ok(())
}

impl Local {
    pub async fn new(cfg: &config::LocalLogin, state_dir: Option<&Path>) -> Result<Self> {
        let hash = match (&cfg.password_hash, state_dir) {
            (Some(s), _) => s
                .resolve()
                .await
                .map_err(|e| format!("local_login.password_hash: {e}"))?,
            (None, Some(dir)) => bootstrap(dir)?,
            (None, None) => {
                return Err(
                    "local_login: set password_hash (homie hash-password) or state_dir".into(),
                );
            }
        };
        PasswordHash::new(&hash)
            .map_err(|e| format!("local_login.password_hash is not an argon2 PHC string: {e}"))?;
        Ok(Self {
            username: cfg.username.clone(),
            hash,
            throttle: Mutex::new(Throttle::default()),
        })
    }

    /// Checks the credentials. After MAX_FAILURES within FAILURE_WINDOW,
    /// every attempt is refused for LOCKOUT.
    pub async fn verify(&self, username: &str, password: &str) -> Result<()> {
        {
            let t = self.throttle.lock().unwrap();
            if let Some(until) = t.locked_until.filter(|u| *u > Instant::now()) {
                let secs = until.duration_since(Instant::now()).as_secs() + 1;
                return Err(format!("too many failed logins, try again in {secs} s").into());
            }
        }
        let hash = self.hash.clone();
        let password = password.to_string();
        // argon2 is deliberately slow: keep it off the async workers
        let ok = tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash).is_ok_and(|h| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &h)
                    .is_ok()
            })
        })
        .await?
            && username == self.username;

        let mut t = self.throttle.lock().unwrap();
        if ok {
            *t = Throttle::default();
            return Ok(());
        }
        let now = Instant::now();
        if t.first
            .is_none_or(|f| now.duration_since(f) > FAILURE_WINDOW)
        {
            *t = Throttle {
                failures: 0,
                first: Some(now),
                locked_until: None,
            };
        }
        t.failures += 1;
        if t.failures >= MAX_FAILURES {
            t.locked_until = Some(now + LOCKOUT);
            tracing::warn!(
                msg = "local login locked",
                failures = t.failures,
                minutes = LOCKOUT.as_secs() / 60
            );
        }
        Err("wrong username or password".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(hash: Option<String>) -> config::LocalLogin {
        config::LocalLogin {
            username: "homie".into(),
            password_hash: hash.map(crate::secret::Secret::Plain),
            role: None,
        }
    }

    #[tokio::test]
    async fn generated_password_is_private_and_reused() {
        let dir = std::env::temp_dir().join(format!("homie-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Local::new(&cfg(None), Some(&dir)).await.unwrap();
        let password = std::fs::read_to_string(dir.join("initial-password")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("initial-password"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(a.verify("homie", password.trim()).await.is_ok());
        // a restart reuses the stored hash instead of generating a new password
        let b = Local::new(&cfg(None), Some(&dir)).await.unwrap();
        assert!(b.verify("homie", password.trim()).await.is_ok());
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            Local::new(&cfg(None), None).await.is_err(),
            "no hash and nowhere to keep one"
        );
        assert!(
            Local::new(&cfg(Some("plain".into())), None).await.is_err(),
            "not a PHC hash"
        );
    }

    #[tokio::test]
    async fn wrong_credentials_and_lockout() {
        let l = Local::new(&cfg(Some(hash("s3cret-long-enough").unwrap())), None)
            .await
            .unwrap();
        assert!(l.verify("homie", "s3cret-long-enough").await.is_ok());
        assert!(l.verify("other", "s3cret-long-enough").await.is_err());
        for _ in 0..MAX_FAILURES {
            let _ = l.verify("homie", "wrong").await;
        }
        let e = l.verify("homie", "s3cret-long-enough").await.unwrap_err();
        assert!(e.to_string().contains("too many failed logins"), "{e}");
    }
}
