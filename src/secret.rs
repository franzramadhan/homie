//! Secret values from the config: a plain string, or a reference to an
//! environment variable, a file or a command (Keychain, pass, op read).

use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize, Serializer};
use tokio::sync::RwLock;

use crate::Result;

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum Secret {
    Env { env: String },
    File { file: PathBuf },
    Cmd { cmd: String },
    Plain(String),
}

impl Secret {
    pub async fn resolve(&self) -> Result<String> {
        let raw = match self {
            Secret::Plain(s) => s.clone(),
            Secret::Env { env } => {
                std::env::var(env).map_err(|_| format!("environment variable {env} is not set"))?
            }
            Secret::File { file } => {
                let path = crate::config::expand(file);
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| format!("reading {}: {e}", path.display()))?
            }
            Secret::Cmd { cmd } => {
                let out = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .output()
                    .await
                    .map_err(|e| format!("running secret command: {e}"))?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    return Err(format!(
                        "secret command failed ({}): {}",
                        out.status,
                        stderr.trim()
                    )
                    .into());
                }
                String::from_utf8(out.stdout).map_err(|_| "secret command printed invalid UTF-8")?
            }
        };
        // a trailing newline from `echo`, a file or a CLI is never part of a token
        let value = raw.trim_end().to_string();
        if value.is_empty() {
            return Err(format!("secret from {self:?} is empty").into());
        }
        Ok(value)
    }
}

/// Never prints the value, only where it comes from.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Secret::Plain(_) => write!(f, "<redacted>"),
            Secret::Env { env } => write!(f, "env:{env}"),
            Secret::File { file } => write!(f, "file:{}", file.display()),
            Secret::Cmd { cmd } => write!(f, "cmd:{cmd}"),
        }
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{self:?}"))
    }
}

/// A resolved secret that can be re-read, for example after a 401 when the
/// token behind a `cmd` or `file` was rotated.
pub struct SecretCell {
    source: Secret,
    value: RwLock<String>,
}

impl SecretCell {
    pub async fn new(source: &Secret) -> Result<Self> {
        Ok(Self {
            value: RwLock::new(source.resolve().await?),
            source: source.clone(),
        })
    }

    pub async fn get(&self) -> String {
        self.value.read().await.clone()
    }

    pub async fn reload(&self) -> Result<()> {
        *self.value.write().await = self.source.resolve().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_and_trims() {
        let s: Secret = serde_json::from_str(r#"{"cmd": "printf 'tok\\n'"}"#).unwrap();
        assert_eq!(s.resolve().await.unwrap(), "tok");
        let s: Secret = serde_json::from_str(r#""plain""#).unwrap();
        assert_eq!(s.resolve().await.unwrap(), "plain");
        assert_eq!(format!("{s:?}"), "<redacted>");
        let s: Secret = serde_json::from_str(r#"{"env": "HOMIE_TEST_UNSET_VAR"}"#).unwrap();
        assert!(s.resolve().await.is_err());
        let s: Secret = serde_json::from_str(r#"{"cmd": "exit 3"}"#).unwrap();
        assert!(s.resolve().await.is_err());
    }
}
