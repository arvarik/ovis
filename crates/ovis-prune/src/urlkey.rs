//! Canonical URL keys and asset classification.
//!
//! Two documents whose URLs differ only by tracking parameters, scheme,
//! `www.`, a trailing slash or a fragment are the same page. Onyx's
//! `content_hash` catches them only when the extraction was byte-identical,
//! which it often is not (a timestamp in the footer is enough to break it).
//! A canonical key catches them structurally.
//!
//! The rule set follows the IIPC `urlcanon` **aggressive** profile
//! (<https://github.com/iipc/urlcanon>), which is the profile built for
//! dedup keys rather than for fetching. Deliberately *not* RFC 3986-pure: it
//! folds https/http and strips `www.`, both of which change what the URL
//! means but not which page it is.
//!
//! The key is never a delete trigger on its own — Dolma's staged measurements
//! (URL dedup removes 53.2% of documents, after which document-level exact
//! dedup still removes 14.9%) show URL identity catches about half the
//! problem, and the inverse case (same URL, genuinely different content after
//! a recrawl) would make it a data-loss trigger. It is a grouping key and a
//! keeper tiebreak.

use serde::{Deserialize, Serialize};

/// Query parameters that never change which page you are looking at.
/// Sourced from the ClearURLs / AdGuard rule lists; the `utm_` family is
/// matched by prefix rather than enumerated.
const TRACKING_PARAMS: [&str; 24] = [
    "fbclid",
    "gclid",
    "gbraid",
    "wbraid",
    "dclid",
    "msclkid",
    "yclid",
    "twclid",
    "ttclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "_ga",
    "_gl",
    "ref_src",
    "ref_url",
    "scid",
    "vero_id",
    "wickedid",
    "oly_anon_id",
    "oly_enc_id",
    "__s",
    "rb_clickid",
    "s_cid",
];

/// Session identifiers: same page, different visitor.
const SESSION_PARAMS: [&str; 6] = [
    "sessionid",
    "session_id",
    "jsessionid",
    "phpsessid",
    "aspsessionid",
    "sid",
];

/// Index filenames that are equivalent to the bare directory.
const INDEX_FILES: [&str; 6] = [
    "index.html",
    "index.htm",
    "index.php",
    "index.asp",
    "index.aspx",
    "default.aspx",
];

/// File extensions that mark a document as a binary asset rather than a page.
/// Onyx indexes these as "documents" when a crawl walks them; their extracted
/// text is usually the filename and dimensions, which is why 60k+ of them
/// collapse into one identical-content hash group on the reference corpus.
const IMAGE_EXTENSIONS: [&str; 12] = [
    "jpg", "jpeg", "png", "gif", "svg", "webp", "bmp", "ico", "tif", "tiff", "avif", "heic",
];

const MEDIA_EXTENSIONS: [&str; 12] = [
    "mp4", "webm", "mov", "avi", "mkv", "mp3", "wav", "ogg", "flac", "m4a", "m4v", "wmv",
];

const ARCHIVE_EXTENSIONS: [&str; 10] = [
    "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "tar", "iso", "dmg",
];

/// Extensions that are documents in their own right — never assets, even
/// though they are binaries. PDFs on this corpus are real content
/// (`crs-reports-mirror`, `bitsavers-tech-archive`), so the classifier reports
/// them separately and lets policy decide.
const DOCUMENT_EXTENSIONS: [&str; 8] = [
    "pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx", "epub",
];

/// What kind of thing a URL points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlClass {
    /// An ordinary page.
    Page,
    Image,
    Media,
    Archive,
    /// PDF and office formats: binary, but genuinely documents.
    BinaryDocument,
}

impl UrlClass {
    pub fn code(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Image => "image",
            Self::Media => "media",
            Self::Archive => "archive",
            Self::BinaryDocument => "binary_document",
        }
    }

    /// Whether this class is an *asset* — something whose indexed text is a
    /// side effect of the crawl rather than content anyone searches for.
    pub fn is_asset(self) -> bool {
        matches!(self, Self::Image | Self::Media | Self::Archive)
    }
}

/// A URL decomposed far enough to canonicalize it. Written by hand rather than
/// with a URL crate because document ids are not always well-formed URLs (some
/// are `file:`-ish paths or carry raw spaces), and a parser that rejects them
/// would silently drop documents from the grouping.
struct Parts<'a> {
    scheme: &'a str,
    host: String,
    port: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
}

fn split<'a>(raw: &'a str) -> Option<Parts<'a>> {
    let without_fragment = raw.split('#').next().unwrap_or(raw);
    let (scheme, rest) = without_fragment.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    // Drop any userinfo — credentials never identify the page.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rfind(':') {
        // Guard against IPv6 literals, whose colons are not a port separator.
        Some(idx) if !authority.contains(']') => (&authority[..idx], Some(&authority[idx + 1..])),
        _ => (authority, None),
    };
    if host.is_empty() {
        return None;
    }
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };
    Some(Parts {
        scheme: scheme.trim(),
        host: host.to_lowercase(),
        port,
        path,
        query,
    })
}

fn is_tracking_param(key: &str) -> bool {
    let lowered = key.to_lowercase();
    lowered.starts_with("utm_")
        || lowered.starts_with("pk_")
        || lowered.starts_with("piwik_")
        || TRACKING_PARAMS.contains(&lowered.as_str())
        || SESSION_PARAMS.contains(&lowered.as_str())
}

/// Collapse `.` and `..` segments and repeated slashes.
fn normalize_path_segments(path: &str) -> String {
    let trailing_slash = path.ends_with('/') && path.len() > 1;
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    // An index file is equivalent to the directory that contains it.
    if let Some(last) = out.last() {
        if INDEX_FILES.contains(&last.to_lowercase().as_str()) {
            out.pop();
        }
    }
    let mut joined = String::with_capacity(path.len());
    for segment in &out {
        joined.push('/');
        joined.push_str(segment);
    }
    if joined.is_empty() {
        joined.push('/');
    } else if trailing_slash {
        // Preserved only so the *display* form stays faithful; the key below
        // strips it, which is what makes `/a` and `/a/` group together.
        joined.push('/');
    }
    joined
}

/// The canonical dedup key for a document URL.
///
/// Folds: scheme (https ≡ http), `www.`/`m.` host prefixes, default ports,
/// fragments, userinfo, `.`/`..` segments, index filenames, trailing slashes,
/// tracking and session parameters, and query-parameter order.
///
/// Returns `None` for input that is not a URL at all, which is the honest
/// answer — such documents simply do not participate in URL grouping.
pub fn canonical_key(raw: &str) -> Option<String> {
    let parts = split(raw.trim())?;

    let scheme = match parts.scheme.to_lowercase().as_str() {
        // The aggressive profile folds these deliberately: an http and https
        // copy of one page are the same page.
        "https" | "http" => "http",
        other => return Some(format!("{other}://{}", parts.host)),
    };

    let mut host = parts.host.as_str();
    for prefix in ["www.", "m.", "mobile.", "amp."] {
        if let Some(stripped) = host.strip_prefix(prefix) {
            host = stripped;
            break;
        }
    }
    let host = host.trim_end_matches('.');

    // Keep only a non-default port.
    let port = match parts.port {
        Some("80") | Some("443") | Some("") | None => String::new(),
        Some(p) => format!(":{p}"),
    };

    let path = normalize_path_segments(parts.path);
    let path = path.trim_end_matches('/');

    let mut query_pairs: Vec<(String, String)> = Vec::new();
    if let Some(query) = parts.query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            if key.is_empty() || is_tracking_param(key) {
                continue;
            }
            query_pairs.push((key.to_lowercase(), value.to_string()));
        }
    }
    // Sorting makes `?a=1&b=2` and `?b=2&a=1` the same key.
    query_pairs.sort();

    let mut key = format!("{scheme}://{host}{port}{path}");
    if !query_pairs.is_empty() {
        key.push('?');
        let rendered: Vec<String> = query_pairs
            .iter()
            .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") })
            .collect();
        key.push_str(&rendered.join("&"));
    }
    Some(key)
}

/// Classify a URL by its file extension.
pub fn classify(raw: &str) -> UrlClass {
    let Some(parts) = split(raw.trim()) else {
        return UrlClass::Page;
    };
    let last_segment = parts.path.rsplit('/').next().unwrap_or("");
    let Some((_, ext)) = last_segment.rsplit_once('.') else {
        return UrlClass::Page;
    };
    let ext = ext.to_lowercase();
    let ext = ext.as_str();
    if IMAGE_EXTENSIONS.contains(&ext) {
        UrlClass::Image
    } else if MEDIA_EXTENSIONS.contains(&ext) {
        UrlClass::Media
    } else if ARCHIVE_EXTENSIONS.contains(&ext) {
        UrlClass::Archive
    } else if DOCUMENT_EXTENSIONS.contains(&ext) {
        UrlClass::BinaryDocument
    } else {
        UrlClass::Page
    }
}

/// How many path segments a URL has — a keeper tiebreak ("shallower wins").
pub fn path_depth(raw: &str) -> usize {
    split(raw.trim())
        .map(|p| p.path.split('/').filter(|s| !s.is_empty()).count())
        .unwrap_or(0)
}

/// Whether the URL carries a query string at all (a keeper tiebreak: the copy
/// without parameters is usually the canonical one).
pub fn has_query(raw: &str) -> bool {
    split(raw.trim()).map(|p| p.query.is_some()).unwrap_or(false)
}

/// Whether a URL looks like a dated archive edition of a live page, e.g.
/// `plato.stanford.edu/archives/spr2011/entries/x` mirroring
/// `plato.stanford.edu/entries/x`. Returns the live-page key when it does.
///
/// Kept narrow on purpose: it recognises an explicit `/archives?/<edition>/`
/// segment, not "any URL containing a year".
pub fn archive_edition_of(raw: &str) -> Option<String> {
    let parts = split(raw.trim())?;
    let segments: Vec<&str> = parts.path.split('/').filter(|s| !s.is_empty()).collect();
    let idx = segments
        .iter()
        .position(|s| *s == "archives" || *s == "archive")?;
    // The segment after `archives` is the edition label; everything past it is
    // the live path.
    let edition = segments.get(idx + 1)?;
    let looks_like_edition = edition.chars().any(|c| c.is_ascii_digit());
    if !looks_like_edition || idx + 2 >= segments.len() {
        return None;
    }
    let live_path = segments[idx + 2..].join("/");
    let host = parts.host.trim_start_matches("www.");
    canonical_key(&format!("{}://{host}/{live_path}", parts.scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_parameters_never_change_the_key() {
        let bare = canonical_key("https://example.com/post").unwrap();
        for variant in [
            "https://example.com/post?utm_source=feed",
            "https://example.com/post?utm_source=feed&utm_medium=rss",
            "https://example.com/post?fbclid=abc123",
            "https://example.com/post?gclid=xyz",
            "https://example.com/post?jsessionid=DEADBEEF",
        ] {
            assert_eq!(canonical_key(variant).unwrap(), bare, "{variant}");
        }
    }

    #[test]
    fn scheme_www_slash_and_fragment_all_fold() {
        let key = canonical_key("https://www.example.com/post/").unwrap();
        for variant in [
            "http://example.com/post",
            "https://example.com/post/",
            "https://www.example.com/post#section-2",
            "https://EXAMPLE.com/post",
            "https://example.com:443/post",
            "https://example.com/./post",
            "https://example.com/other/../post",
            "https://example.com/post/index.html",
        ] {
            assert_eq!(canonical_key(variant).unwrap(), key, "{variant}");
        }
    }

    #[test]
    fn meaningful_query_parameters_are_kept_and_order_normalized() {
        let a = canonical_key("https://example.com/search?q=rust&page=2").unwrap();
        let b = canonical_key("https://example.com/search?page=2&q=rust").unwrap();
        assert_eq!(a, b, "parameter order must not matter");
        assert!(a.contains("q=rust"), "{a}");
        assert!(a.contains("page=2"), "{a}");
        assert_ne!(
            a,
            canonical_key("https://example.com/search?q=go&page=2").unwrap(),
            "different values are different pages"
        );
    }

    #[test]
    fn distinct_pages_keep_distinct_keys() {
        let keys = [
            "https://example.com/a",
            "https://example.com/b",
            "https://other.com/a",
            "https://example.com/a/deeper",
        ]
        .map(|u| canonical_key(u).unwrap());
        let mut unique = keys.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "no collisions across real pages");
    }

    #[test]
    fn the_lang_variant_from_the_live_corpus_folds() {
        // Observed on gamma: esa.int serves the same page under a (lang)
        // suffix. These differ by a path segment, so they must NOT fold —
        // this test pins that we do not over-collapse.
        let a = canonical_key("https://www.esa.int/ESA_Multimedia/Videos/2020/05/A_future").unwrap();
        let b = canonical_key(
            "https://www.esa.int/ESA_Multimedia/Videos/2020/05/A_future/(lang)/en",
        )
        .unwrap();
        assert_ne!(a, b, "a path difference is a different URL; only policy may equate them");
    }

    #[test]
    fn non_urls_are_reported_honestly_rather_than_guessed() {
        assert!(canonical_key("not a url").is_none());
        assert!(canonical_key("").is_none());
        assert!(canonical_key("https://").is_none());
    }

    #[test]
    fn assets_are_classified_and_pdfs_are_not_assets() {
        assert_eq!(classify("https://a.com/cat.png"), UrlClass::Image);
        assert_eq!(classify("https://a.com/x/y/photo.JPEG"), UrlClass::Image);
        assert_eq!(classify("https://a.com/clip.mp4"), UrlClass::Media);
        assert_eq!(classify("https://a.com/dump.tar.gz"), UrlClass::Archive);
        assert_eq!(classify("https://a.com/report.pdf"), UrlClass::BinaryDocument);
        assert_eq!(classify("https://a.com/page"), UrlClass::Page);
        assert_eq!(classify("https://a.com/"), UrlClass::Page);

        assert!(UrlClass::Image.is_asset());
        assert!(!UrlClass::BinaryDocument.is_asset(), "PDFs are real content here");
        assert!(!UrlClass::Page.is_asset());
    }

    #[test]
    fn a_query_string_does_not_confuse_the_extension_classifier() {
        assert_eq!(classify("https://a.com/cat.png?size=large"), UrlClass::Image);
    }

    #[test]
    fn archive_editions_resolve_to_the_live_page() {
        let live = archive_edition_of(
            "https://plato.stanford.edu/archives/spr2011/entries/ethics-deontological/",
        )
        .unwrap();
        assert_eq!(
            live,
            canonical_key("https://plato.stanford.edu/entries/ethics-deontological/").unwrap()
        );
        // Not every path containing "archive" is an edition mirror.
        assert!(archive_edition_of("https://a.com/archive/").is_none());
        assert!(archive_edition_of("https://a.com/blog/archives/tag/rust").is_none());
    }

    #[test]
    fn path_depth_and_query_presence_serve_the_keeper_tiebreaks() {
        assert_eq!(path_depth("https://a.com/"), 0);
        assert_eq!(path_depth("https://a.com/x/y/z"), 3);
        assert!(has_query("https://a.com/x?a=1"));
        assert!(!has_query("https://a.com/x"));
    }

    #[test]
    fn userinfo_and_ipv6_hosts_do_not_break_parsing() {
        assert_eq!(
            canonical_key("https://user:pw@example.com/x").unwrap(),
            canonical_key("https://example.com/x").unwrap()
        );
        assert!(canonical_key("http://[2001:db8::1]/x").is_some());
    }
}
