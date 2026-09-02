use std::path::{Path, PathBuf};

use tracing::debug;

const ENV_VAR: &str = "SYNCTHING_API_KEY";

/// Work out the Syncthing API key, in descending order of explicitness:
///
/// 1. `--api-key`, or `SYNCTHING_API_KEY` in the environment (both arrive here
///    already resolved by clap)
/// 2. `SYNCTHING_API_KEY` in `~/.env`
/// 3. the `<apikey>` Syncthing wrote into its own config
///
/// Steps 2 and 3 exist so tray mode works when launched from a desktop entry,
/// where the shell environment a terminal would have provided is absent.
pub fn resolve(from_cli: Option<String>) -> Option<String> {
    if let Some(key) = from_cli {
        debug!("Using API key from --api-key/{ENV_VAR}");
        return Some(key);
    }

    if let Some(key) = from_home_dotenv() {
        debug!("Using API key from ~/.env");
        return Some(key);
    }

    if let Some((key, path)) = from_syncthing_config() {
        debug!("Using API key from {}", path.display());
        return Some(key);
    }

    None
}

/// Read `SYNCTHING_API_KEY` out of `~/.env`.
///
/// Deliberately uses the iterator form rather than `dotenvy::from_path`, which
/// would load every variable in the file into the process environment. We only
/// want this one key, not the user's unrelated shell exports.
fn from_home_dotenv() -> Option<String> {
    read_dotenv(&dirs::home_dir()?.join(".env"))
}

fn read_dotenv(path: &Path) -> Option<String> {
    dotenvy::from_path_iter(path)
        .ok()?
        .flatten()
        .find(|(name, _)| name == ENV_VAR)
        .map(|(_, value)| value)
}

/// Fall back to the API key Syncthing generated for itself.
fn from_syncthing_config() -> Option<(String, PathBuf)> {
    for path in syncthing_config_paths() {
        let Ok(xml) = std::fs::read_to_string(&path) else {
            continue;
        };

        if let Some(key) = extract_api_key(&xml) {
            return Some((key.to_string(), path));
        }
    }

    None
}

/// Recent Syncthing versions store their config under `XDG_STATE_HOME`; older
/// ones used `XDG_CONFIG_HOME`, and that is still where it lives on macOS and
/// Windows.
fn syncthing_config_paths() -> Vec<PathBuf> {
    [dirs::state_dir(), dirs::config_dir()]
        .into_iter()
        .flatten()
        .map(|dir| dir.join("syncthing").join("config.xml"))
        .collect()
}

/// Pull the contents of the single `<apikey>` element out of Syncthing's config.
///
/// Scanning for the tags by hand rather than adding an XML parser: the key is
/// always plain alphanumeric, so there are no entities or attributes to decode,
/// and this is the only element we will ever read from the file.
fn extract_api_key(xml: &str) -> Option<&str> {
    const OPEN: &str = "<apikey>";
    const CLOSE: &str = "</apikey>";

    let after_open = &xml[xml.find(OPEN)? + OPEN.len()..];
    let key = after_open[..after_open.find(CLOSE)?].trim();

    (!key.is_empty()).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_api_key() {
        let xml = r#"<configuration version="37">
            <gui enabled="true">
                <address>127.0.0.1:8384</address>
                <apikey>abc123XYZ</apikey>
            </gui>
        </configuration>"#;

        assert_eq!(extract_api_key(xml), Some("abc123XYZ"));
    }

    #[test]
    fn reads_the_key_from_a_dotenv_file_including_the_export_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        // A name the ambient environment cannot plausibly already hold, so the
        // pollution assertion below is testing us and not the developer's shell.
        let unrelated = "TIDYSYNC_TEST_UNRELATED_VAR";
        std::fs::write(
            &path,
            format!("export {unrelated}=somevalue\nexport SYNCTHING_API_KEY=fromdotenv\n"),
        )
        .unwrap();

        assert_eq!(read_dotenv(&path), Some("fromdotenv".to_string()));

        // Reading must not import the file's other variables into our process.
        assert!(std::env::var(unrelated).is_err());
    }

    #[test]
    fn returns_none_for_a_missing_dotenv_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_dotenv(&dir.path().join("nope.env")), None);
    }

    #[test]
    fn returns_none_when_the_element_is_absent_or_empty() {
        assert_eq!(extract_api_key("<configuration></configuration>"), None);
        assert_eq!(extract_api_key("<apikey></apikey>"), None);
        assert_eq!(extract_api_key("<apikey>   </apikey>"), None);
        assert_eq!(extract_api_key("<apikey>truncated"), None);
    }
}
