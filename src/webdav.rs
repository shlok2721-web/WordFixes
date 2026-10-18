//! Minimal WebDAV client — the transport half of client-mode sync.
//!
//! Deliberately not a general WebDAV library. It speaks the five verbs this app
//! needs against a server that stores the same `notes/*.json` tree the phone
//! syncs with, and nothing else.

use anyhow::{Context, Result};
use base64::Engine;

pub struct Client {
    base: String,
    auth: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    /// `base` is the sync root, e.g. `https://dav.example.org/notes-vault`.
    pub fn new(base: &str, username: &str, password: &str) -> Self {
        let auth = (!username.is_empty()).then(|| {
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
            )
        });
        Self {
            base: base.trim_end_matches('/').to_string(),
            auth,
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = self.agent.request(method, &self.url(path));
        match &self.auth {
            Some(a) => r.set("Authorization", a),
            None => r,
        }
    }

    /// Lists every `.json` file under `path`, walking subfolders.
    ///
    /// One request per collection, because `Depth: infinity` is refused (403) by
    /// this app's own server and commonly by Apache and nginx too. A notes vault
    /// is one folder level deep, so this is a handful of requests.
    pub fn list_json(&self, path: &str) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let mut queue = vec![path.trim_matches('/').to_string()];
        while let Some(dir) = queue.pop() {
            let body = self
                .req("PROPFIND", &dir)
                .set("Depth", "1")
                .send_string(PROPFIND_BODY)
                .with_context(|| format!("PROPFIND {dir}"))?
                .into_string()?;
            for href in hrefs(&body) {
                let Some(rel) = self.relative(&href) else {
                    continue;
                };
                // Depth 1 includes the collection itself; queueing it would loop.
                if rel.trim_end_matches('/') == dir {
                    continue;
                }
                if href.ends_with('/') {
                    queue.push(rel.trim_end_matches('/').to_string());
                } else if rel.ends_with(".json") {
                    files.push(rel);
                }
            }
        }
        Ok(files)
    }

    /// Turns an absolute or root-relative href from a PROPFIND response back
    /// into a path relative to the sync root.
    fn relative(&self, href: &str) -> Option<String> {
        let tail = href.strip_prefix(&self.base).or_else(|| {
            // Servers usually answer with a root-relative href, so compare
            // against the base's path component rather than the whole URL. A
            // base that is bare host:port (`http://127.0.0.1:8080`) has no path
            // component and every href is already relative to it.
            let base_path = self
                .base
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(_, path)| path)
                .unwrap_or("");
            href.trim_start_matches('/')
                .strip_prefix(base_path)
                .map(|t| t.trim_start_matches('/'))
        })?;
        let tail = tail.trim_start_matches('/');
        (!tail.is_empty()).then(|| percent_decode(tail))
    }

    pub fn get(&self, path: &str) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.req("GET", path)
            .call()
            .with_context(|| format!("GET {path}"))?
            .into_reader()
            .read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Uploads `body`, creating any missing parent collections first — a folder
    /// that only exists locally has no directory on the server yet.
    pub fn put(&self, path: &str, body: &[u8]) -> Result<()> {
        if let Some((dir, _)) = path.rsplit_once('/') {
            self.mkcol_all(dir)?;
        }
        self.req("PUT", path)
            .send_bytes(body)
            .with_context(|| format!("PUT {path}"))?;
        Ok(())
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        match self.req("DELETE", path).call() {
            // Already gone is the outcome we wanted.
            Err(ureq::Error::Status(404 | 410, _)) => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("DELETE {path}"))),
            Ok(_) => Ok(()),
        }
    }

    fn mkcol_all(&self, dir: &str) -> Result<()> {
        let mut so_far = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            if !so_far.is_empty() {
                so_far.push('/');
            }
            so_far.push_str(part);
            match self.req("MKCOL", &so_far).call() {
                // 405 = it already exists, which is the normal case.
                Ok(_) | Err(ureq::Error::Status(405, _)) => {}
                Err(e) => return Err(anyhow::Error::new(e).context(format!("MKCOL {so_far}"))),
            }
        }
        Ok(())
    }
}

const PROPFIND_BODY: &str =
    r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop/></d:propfind>"#;

/// Pulls the `<href>` values out of a PROPFIND multistatus response.
///
/// ponytail: string scan, not an XML parse — the only thing we need from the
/// response is hrefs, and the namespace prefix varies by server (`d:`, `D:`,
/// `lp1:`) which a naive tag match handles fine. Swap in a real XML parser if
/// this ever has to read properties.
fn hrefs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find("href>") {
        rest = &rest[open + "href>".len()..];
        let Some(close) = rest.find("</") else { break };
        let value = rest[..close].trim();
        if !value.is_empty() {
            out.push(value.to_string());
        }
        // Past the closing tag, whose own "href>" would otherwise match again.
        rest = &rest[close..];
        match rest.find("href>") {
            Some(end) => rest = &rest[end + "href>".len()..],
            None => break,
        }
    }
    out
}

/// Decodes the `%XX` escapes a server puts in hrefs. Folder names here are
/// routinely Greek, so this is not optional.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_survive_any_namespace_prefix() {
        let xml = r#"<D:multistatus xmlns:D="DAV:">
            <D:response><D:href>/vault/notes/</D:href></D:response>
            <D:response><lp1:href>/vault/notes/a.json</lp1:href></D:response>
        </D:multistatus>"#;
        assert_eq!(hrefs(xml), vec!["/vault/notes/", "/vault/notes/a.json"]);
    }

    #[test]
    fn greek_folder_names_round_trip() {
        assert_eq!(
            percent_decode("notes/%CE%A3%CF%87%CE%BF%CE%BB%CE%B5%CE%AF%CE%BF/a.json"),
            "notes/Σχολείο/a.json"
        );
    }

    #[test]
    fn hrefs_become_paths_relative_to_the_sync_root() {
        let c = Client::new("https://example.org/vault", "", "");
        assert_eq!(
            c.relative("/vault/notes/a.json").as_deref(),
            Some("notes/a.json")
        );
        assert_eq!(
            c.relative("https://example.org/vault/notes/a.json")
                .as_deref(),
            Some("notes/a.json")
        );
        // The collection itself is not a note.
        assert_eq!(c.relative("/vault/").as_deref(), None);
    }
}
