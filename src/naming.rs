//! Friendly-name, password and free-port helpers for the "Automatic" server
//! setup. The goal is a non-technical user can flip one switch and get
//! copy-pasteable connection details, LocalSend-style.

use rand::seq::IndexedRandom;

const ADJECTIVES: &[&str] = &[
    "happy", "angry", "sleepy", "brave", "clever", "gentle", "jolly", "lucky", "mighty", "silent",
    "swift", "witty", "calm", "bold", "shiny", "cosmic", "fuzzy", "sunny", "breezy", "quiet",
];

const NOUNS: &[&str] = &[
    "watermelon",
    "banana",
    "otter",
    "panda",
    "falcon",
    "cactus",
    "walnut",
    "dolphin",
    "maple",
    "comet",
    "pepper",
    "lantern",
    "pebble",
    "willow",
    "raccoon",
    "acorn",
    "meadow",
    "harbor",
    "cricket",
    "sparrow",
];

/// e.g. `"happy-watermelon"`. Used as the WebDAV username so the phone side
/// shows something memorable rather than `noteuser`.
pub fn friendly_name() -> String {
    let mut rng = rand::rng();
    format!(
        "{}-{}",
        ADJECTIVES.choose(&mut rng).unwrap(),
        NOUNS.choose(&mut rng).unwrap()
    )
}

/// A readable random password. Avoids look-alike characters (0/O, 1/l/I) so it
/// can be typed on a phone without confusion.
pub fn random_password(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyzACDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| *CHARS.choose(&mut rng).unwrap() as char)
        .collect()
}

/// Returns the first bindable port at or after `preferred`, scanning a small
/// range. `None` if nothing in range is free. Binding then dropping the
/// listener is the only reliable "is it free" test.
pub fn find_available_port(preferred: u16) -> Option<u16> {
    (preferred..preferred.saturating_add(50)).find(|&port| port_is_free(port))
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// Identifier of the network this machine is currently on — the active
/// NetworkManager connection name (usually the Wi-Fi SSID), falling back to
/// `iwgetid`. `None` when neither tool answers (offline, or neither
/// installed). Used to key the per-network server profiles.
pub fn current_network() -> Option<String> {
    let from_nmcli = std::process::Command::new("nmcli")
        .args(["-t", "-f", "NAME,TYPE", "connection", "show", "--active"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.ends_with(":loopback"))
                .filter_map(|l| l.rsplit_once(':').map(|(name, _)| name.to_string()))
                .next()
        });
    from_nmcli
        .or_else(|| {
            std::process::Command::new("iwgetid")
                .arg("-r")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_name_is_adjective_dash_noun() {
        let name = friendly_name();
        let (adj, noun) = name.split_once('-').expect("has a dash");
        assert!(ADJECTIVES.contains(&adj), "{adj} not an adjective");
        assert!(NOUNS.contains(&noun), "{noun} not a noun");
    }

    #[test]
    fn password_has_the_requested_length_and_no_lookalikes() {
        let pw = random_password(16);
        assert_eq!(pw.chars().count(), 16);
        assert!(
            !pw.contains(['0', 'O', '1', 'l', 'I']),
            "contains a look-alike: {pw}"
        );
    }

    #[test]
    fn finds_a_free_port() {
        // Some port in the range is essentially always free in a test env.
        let port = find_available_port(49200).expect("a free port exists");
        assert!((49200..49250).contains(&port));
        // And it really is bindable.
        assert!(std::net::TcpListener::bind(("0.0.0.0", port)).is_ok());
    }

    #[test]
    fn a_taken_port_is_skipped() {
        // Hold a port, then confirm the finder moves past it.
        let held = std::net::TcpListener::bind(("0.0.0.0", 49300)).unwrap();
        let port = find_available_port(49300).unwrap();
        assert_ne!(port, 49300, "should skip the held port");
        drop(held);
    }
}
