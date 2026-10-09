//! The preview table: file name -> what a browser can render it as.
//!
//! Only types a browser can actually render inline are listed, because that is
//! the whole of what a preview can be here — there is no server-side converter:
//!
//! - `Image`/`Video`/`Audio` are streamed raw and put in an `<img>`, `<video>`
//!   or `<audio>`;
//! - `Pdf` is streamed raw into an `<iframe>` (the browser's own viewer);
//! - `Text` is read (truncated) and returned as JSON.
//!
//! So `image/tiff` and `image/heic` are deliberately absent — no browser
//! decodes them in an `<img>` — and `video/x-msvideo` is absent because nothing
//! plays AVI.  Office documents (docx/xlsx/pptx) are zipped XML and would need
//! a converter, so they are not previewable either.
//!
//! A file whose extension is not in the table is not advertised as previewable,
//! but the server still tries it as text, so an oddly-named plain-text file
//! previews when fetched directly.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
}

/// (extension, kind, content type).  The content type is what the server sends
/// for media; for text it is only the fallback the browser gets if the file is
/// fetched directly.
pub const TYPES: &[(&str, Kind, &str)] = &[
    // --- images the browser decodes itself ---
    ("png", Kind::Image, "image/png"),
    ("jpg", Kind::Image, "image/jpeg"),
    ("jpeg", Kind::Image, "image/jpeg"),
    ("gif", Kind::Image, "image/gif"),
    ("webp", Kind::Image, "image/webp"),
    ("bmp", Kind::Image, "image/bmp"),
    ("avif", Kind::Image, "image/avif"),
    ("ico", Kind::Image, "image/x-icon"),
    // SVG is XML and can carry script, but the SPA only ever renders it through
    // an <img>, which cannot run script.  The CSP header the server puts on the
    // response covers the case where the URL is opened as a document instead.
    ("svg", Kind::Image, "image/svg+xml"),
    // --- video the browser can play ---
    ("mp4", Kind::Video, "video/mp4"),
    ("m4v", Kind::Video, "video/mp4"),
    ("webm", Kind::Video, "video/webm"),
    ("mkv", Kind::Video, "video/x-matroska"),
    ("mov", Kind::Video, "video/quicktime"),
    ("ogv", Kind::Video, "video/ogg"),
    // --- audio ---
    ("mp3", Kind::Audio, "audio/mpeg"),
    ("wav", Kind::Audio, "audio/wav"),
    ("ogg", Kind::Audio, "audio/ogg"),
    ("oga", Kind::Audio, "audio/ogg"),
    ("opus", Kind::Audio, "audio/opus"),
    ("flac", Kind::Audio, "audio/flac"),
    ("m4a", Kind::Audio, "audio/mp4"),
    ("aac", Kind::Audio, "audio/aac"),
    // --- documents the browser renders natively ---
    ("pdf", Kind::Pdf, "application/pdf"),
    // --- text: source, config, data, subtitles ---
    ("txt", Kind::Text, "text/plain; charset=utf-8"),
    ("md", Kind::Text, "text/plain; charset=utf-8"),
    ("markdown", Kind::Text, "text/plain; charset=utf-8"),
    ("rst", Kind::Text, "text/plain; charset=utf-8"),
    ("adoc", Kind::Text, "text/plain; charset=utf-8"),
    ("log", Kind::Text, "text/plain; charset=utf-8"),
    ("csv", Kind::Text, "text/plain; charset=utf-8"),
    ("tsv", Kind::Text, "text/plain; charset=utf-8"),
    ("json", Kind::Text, "text/plain; charset=utf-8"),
    ("jsonc", Kind::Text, "text/plain; charset=utf-8"),
    ("jsonl", Kind::Text, "text/plain; charset=utf-8"),
    ("ndjson", Kind::Text, "text/plain; charset=utf-8"),
    ("yaml", Kind::Text, "text/plain; charset=utf-8"),
    ("yml", Kind::Text, "text/plain; charset=utf-8"),
    ("toml", Kind::Text, "text/plain; charset=utf-8"),
    ("ini", Kind::Text, "text/plain; charset=utf-8"),
    ("cfg", Kind::Text, "text/plain; charset=utf-8"),
    ("conf", Kind::Text, "text/plain; charset=utf-8"),
    ("config", Kind::Text, "text/plain; charset=utf-8"),
    ("env", Kind::Text, "text/plain; charset=utf-8"),
    ("properties", Kind::Text, "text/plain; charset=utf-8"),
    ("xml", Kind::Text, "text/plain; charset=utf-8"),
    ("html", Kind::Text, "text/plain; charset=utf-8"),
    ("htm", Kind::Text, "text/plain; charset=utf-8"),
    ("xhtml", Kind::Text, "text/plain; charset=utf-8"),
    ("css", Kind::Text, "text/plain; charset=utf-8"),
    ("scss", Kind::Text, "text/plain; charset=utf-8"),
    ("sass", Kind::Text, "text/plain; charset=utf-8"),
    ("less", Kind::Text, "text/plain; charset=utf-8"),
    ("js", Kind::Text, "text/plain; charset=utf-8"),
    ("mjs", Kind::Text, "text/plain; charset=utf-8"),
    ("cjs", Kind::Text, "text/plain; charset=utf-8"),
    ("jsx", Kind::Text, "text/plain; charset=utf-8"),
    ("ts", Kind::Text, "text/plain; charset=utf-8"),
    ("tsx", Kind::Text, "text/plain; charset=utf-8"),
    ("vue", Kind::Text, "text/plain; charset=utf-8"),
    ("svelte", Kind::Text, "text/plain; charset=utf-8"),
    ("sh", Kind::Text, "text/plain; charset=utf-8"),
    ("bash", Kind::Text, "text/plain; charset=utf-8"),
    ("zsh", Kind::Text, "text/plain; charset=utf-8"),
    ("ps1", Kind::Text, "text/plain; charset=utf-8"),
    ("bat", Kind::Text, "text/plain; charset=utf-8"),
    ("cmd", Kind::Text, "text/plain; charset=utf-8"),
    ("py", Kind::Text, "text/plain; charset=utf-8"),
    ("pyi", Kind::Text, "text/plain; charset=utf-8"),
    ("rb", Kind::Text, "text/plain; charset=utf-8"),
    ("pl", Kind::Text, "text/plain; charset=utf-8"),
    ("php", Kind::Text, "text/plain; charset=utf-8"),
    ("lua", Kind::Text, "text/plain; charset=utf-8"),
    ("r", Kind::Text, "text/plain; charset=utf-8"),
    ("jl", Kind::Text, "text/plain; charset=utf-8"),
    ("rs", Kind::Text, "text/plain; charset=utf-8"),
    ("c", Kind::Text, "text/plain; charset=utf-8"),
    ("h", Kind::Text, "text/plain; charset=utf-8"),
    ("cpp", Kind::Text, "text/plain; charset=utf-8"),
    ("hpp", Kind::Text, "text/plain; charset=utf-8"),
    ("go", Kind::Text, "text/plain; charset=utf-8"),
    ("java", Kind::Text, "text/plain; charset=utf-8"),
    ("kt", Kind::Text, "text/plain; charset=utf-8"),
    ("scala", Kind::Text, "text/plain; charset=utf-8"),
    ("hs", Kind::Text, "text/plain; charset=utf-8"),
    ("elm", Kind::Text, "text/plain; charset=utf-8"),
    ("ex", Kind::Text, "text/plain; charset=utf-8"),
    ("exs", Kind::Text, "text/plain; charset=utf-8"),
    ("erl", Kind::Text, "text/plain; charset=utf-8"),
    ("zig", Kind::Text, "text/plain; charset=utf-8"),
    ("nim", Kind::Text, "text/plain; charset=utf-8"),
    ("cr", Kind::Text, "text/plain; charset=utf-8"),
    ("d", Kind::Text, "text/plain; charset=utf-8"),
    ("vala", Kind::Text, "text/plain; charset=utf-8"),
    ("groovy", Kind::Text, "text/plain; charset=utf-8"),
    ("sql", Kind::Text, "text/plain; charset=utf-8"),
    ("nix", Kind::Text, "text/plain; charset=utf-8"),
    ("tf", Kind::Text, "text/plain; charset=utf-8"),
    ("hcl", Kind::Text, "text/plain; charset=utf-8"),
    ("proto", Kind::Text, "text/plain; charset=utf-8"),
    ("graphql", Kind::Text, "text/plain; charset=utf-8"),
    ("cmake", Kind::Text, "text/plain; charset=utf-8"),
    ("mk", Kind::Text, "text/plain; charset=utf-8"),
    ("gradle", Kind::Text, "text/plain; charset=utf-8"),
    ("patch", Kind::Text, "text/plain; charset=utf-8"),
    ("diff", Kind::Text, "text/plain; charset=utf-8"),
    ("service", Kind::Text, "text/plain; charset=utf-8"),
    ("socket", Kind::Text, "text/plain; charset=utf-8"),
    ("timer", Kind::Text, "text/plain; charset=utf-8"),
    ("desktop", Kind::Text, "text/plain; charset=utf-8"),
    ("plist", Kind::Text, "text/plain; charset=utf-8"),
    ("tex", Kind::Text, "text/plain; charset=utf-8"),
    ("bib", Kind::Text, "text/plain; charset=utf-8"),
    ("ipynb", Kind::Text, "text/plain; charset=utf-8"),
    ("srt", Kind::Text, "text/plain; charset=utf-8"),
    ("vtt", Kind::Text, "text/plain; charset=utf-8"),
    ("ics", Kind::Text, "text/plain; charset=utf-8"),
    ("vcf", Kind::Text, "text/plain; charset=utf-8"),
    ("eml", Kind::Text, "text/plain; charset=utf-8"),
    ("mbox", Kind::Text, "text/plain; charset=utf-8"),
    ("lock", Kind::Text, "text/plain; charset=utf-8"),
    ("mod", Kind::Text, "text/plain; charset=utf-8"),
    ("sum", Kind::Text, "text/plain; charset=utf-8"),
    ("gitignore", Kind::Text, "text/plain; charset=utf-8"),
    ("dockerignore", Kind::Text, "text/plain; charset=utf-8"),
    ("editorconfig", Kind::Text, "text/plain; charset=utf-8"),
    ("htaccess", Kind::Text, "text/plain; charset=utf-8"),
    ("npmrc", Kind::Text, "text/plain; charset=utf-8"),
];

/// Files with no extension but a well-known name (`Makefile`, `Dockerfile`,
/// `LICENSE`).  `extension()` cannot see these, so they are matched on the
/// whole name.
pub const NAMES: &[(&str, Kind)] = &[
    ("makefile", Kind::Text),
    ("dockerfile", Kind::Text),
    ("justfile", Kind::Text),
    ("license", Kind::Text),
    ("readme", Kind::Text),
    ("changelog", Kind::Text),
    ("contributing", Kind::Text),
    ("notice", Kind::Text),
];

/// Lowercased extension, or None for a name with no dot (`Makefile`).
pub fn extension(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, e)| e)
}

/// What the browser can render this file as, or None if it cannot.
pub fn kind(name: &str) -> Option<Kind> {
    let lower = name.to_lowercase();
    match extension(&lower) {
        Some(ext) => TYPES.iter().find(|(e, _, _)| *e == ext).map(|(_, k, _)| *k),
        None => NAMES.iter().find(|(n, _)| *n == lower).map(|(_, k)| *k),
    }
}

/// Content type to send for this file.  Media responses use it directly; text
/// responses are JSON, so this is only what a direct fetch gets.
pub fn content_type(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    match extension(&lower) {
        Some(ext) => TYPES.iter().find(|(e, _, _)| *e == ext).map(|(_, _, m)| *m),
        None => NAMES.iter().find(|(n, _)| *n == lower).map(|(_, k)| match k {
            Kind::Text => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind("photo.PNG"), Some(Kind::Image));
        assert_eq!(kind("clip.mp4"), Some(Kind::Video));
        assert_eq!(kind("song.flac"), Some(Kind::Audio));
        assert_eq!(kind("report.pdf"), Some(Kind::Pdf));
        assert_eq!(kind("notes.txt"), Some(Kind::Text));
    }

    #[test]
    fn extension_less_names() {
        assert_eq!(kind("Makefile"), Some(Kind::Text));
        assert_eq!(kind("Dockerfile"), Some(Kind::Text));
        assert_eq!(kind("LICENSE"), Some(Kind::Text));
        // a dot in a directory name must not be read as an extension
        assert_eq!(extension("no-extension"), None);
    }

    #[test]
    fn browser_undecodable_types_are_not_listed() {
        // image/tiff and video/x-msvideo are real MIME types but nothing renders
        // them, so the UI must not promise a preview for them.
        assert_eq!(kind("scan.tiff"), None);
        assert_eq!(kind("movie.avi"), None);
        assert_eq!(kind("document.docx"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("a.svg"), Some("image/svg+xml"));
        assert_eq!(content_type("a.mkv"), Some("video/x-matroska"));
        assert_eq!(content_type("a.opus"), Some("audio/opus"));
        assert_eq!(content_type("a.pdf"), Some("application/pdf"));
        // unknown: the server falls back to mime_guess
        assert_eq!(content_type("unknown-thing"), None);
    }
}
