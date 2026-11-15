//! Nothing a document names outside itself is reached: no connection to a loopback listener, no
//! read of a local canary file, no connection for a UNC path; and the API takes no path at all.
//! Each case runs a positive control first, so a test cannot pass because the document's
//! pictures were never read in the first place.

mod common;

use std::net::TcpStream;
use std::path::Path;

use beebeeb_office::{DocKind, Document, Limits};
use common::{Listener, canary_pixels, canary_png, file_url, linked_docx, render};

/// The same document with its three pictures embedded draws the canary: the document's picture
/// markup is read and drawn, so a hostile variant showing no canary means the target was not
/// resolved, not that the picture was skipped.
fn control_draws_the_canary() {
    let control = linked_docx("media/canary.png", false, true);
    let doc = Document::open(DocKind::Docx, &control, &Limits::default()).expect("the control opens");
    let drawn: usize = (0..doc.info().pages).map(|p| canary_pixels(&render(&doc, p, 1.0).1)).sum();
    // Two of the three pictures (the DrawingML and the VML one) embed it at 144 x 144 pt.
    assert!(drawn > 10_000, "the control draws the canary: {drawn} magenta pixels");
}

/// Open `bytes` and render every page: no page shows the canary, and each says content is
/// missing (the linked pictures were left out).
fn renders_without_reaching(what: &str, bytes: &[u8]) {
    let doc = Document::open(DocKind::Docx, bytes, &Limits::default()).unwrap_or_else(|e| panic!("{what}: opens ({e})"));
    for page in 0..doc.info().pages {
        let (info, px) = render(&doc, page, 1.0);
        assert!(info.missing_content, "{what}: page {page} says content is missing");
        assert_eq!(canary_pixels(&px), 0, "{what}: page {page} shows no canary");
    }
}

#[test]
fn a_remote_picture_is_not_fetched() {
    let listener = Listener::bind("127.0.0.1:0").expect("listen on loopback");
    // The instrument counts: one connection from here is seen, then the count starts again.
    drop(TcpStream::connect(listener.addr).expect("connect to the listener"));
    assert_eq!(listener.drain(), 1, "the listener counts a connection");
    control_draws_the_canary();
    let url = format!("http://{}/x.png", listener.addr);
    renders_without_reaching("a document linking a loopback URL", &linked_docx(&url, true, false));
    assert_eq!(listener.drain(), 0, "no connection to {url}");
}

#[test]
fn a_local_file_target_is_not_read() {
    let dir = std::env::temp_dir().join(format!("bb-office-canary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let canary = dir.join("canary.png");
    std::fs::write(&canary, canary_png()).expect("write the canary");
    control_draws_the_canary();
    let url = file_url(&canary);
    let raw = canary.display().to_string();
    let escape = format!("{}{}", "../".repeat(16), raw.trim_start_matches('/'));
    for (what, target, external) in [
        ("an external file: URL", url.as_str(), true),
        ("an external bare path", raw.as_str(), true),
        ("an internal file: URL", url.as_str(), false),
        ("an internal path climbing out of the package", escape.as_str(), false),
    ] {
        renders_without_reaching(what, &linked_docx(target, external, false));
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_unc_target_makes_no_connection() {
    // SMB on 445, NetBIOS on 139, and the WebDAV fallback on 80: count whichever can be bound.
    let mut listening = Vec::new();
    let mut not_bound = Vec::new();
    for port in [445u16, 139, 80] {
        match Listener::bind(&format!("127.0.0.1:{port}")) {
            Ok(l) => listening.push((port, l)),
            Err(e) => not_bound.push((port, e.kind())),
        }
    }
    for (port, l) in &listening {
        drop(TcpStream::connect(l.addr).expect("connect to the listener"));
        assert_eq!(l.drain(), 1, "the listener on port {port} counts a connection");
    }
    control_draws_the_canary();
    for target in [r"\\127.0.0.1\share\x.png", "file://127.0.0.1/share/x.png", "//127.0.0.1/share/x.png"] {
        renders_without_reaching(target, &linked_docx(target, true, false));
    }
    for (port, l) in &listening {
        assert_eq!(l.drain(), 0, "no connection to port {port}");
    }
    if let Some((_, kind)) = not_bound.iter().find(|(port, _)| *port == 445) {
        // Not a pass for the connection claim: the line is counted as a skip by name.
        eprintln!("SKIPPED a_unc_target_makes_no_connection: no listener on 127.0.0.1:445 ({kind:?}); the connection count was not taken");
    }
}

/// Every `pub fn` in `src/`, its parameter list (comment lines left out).
fn public_fn_params(src: &str) -> Vec<(String, String)> {
    let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let mut out = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find("pub fn ") {
        let sig = &rest[at + "pub fn ".len()..];
        let name: String = sig.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let Some(open) = sig.find('(') else { break };
        let mut depth = 0usize;
        let mut close = None;
        for (i, c) in sig[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        out.push((name, sig[open + 1..close].to_string()));
        rest = &sig[close..];
    }
    out
}

#[test]
fn the_api_takes_no_path() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = 0;
    let mut fns = Vec::new();
    let mut io = Vec::new();
    for entry in std::fs::read_dir(&src).expect("read src/") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        files += 1;
        let text = std::fs::read_to_string(&path).expect("read a source file");
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        for (name, params) in public_fn_params(&text) {
            fns.push((file.clone(), name, params));
        }
        // Nothing in the crate opens a file, a socket or a process.
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for banned in ["std::fs", "std::net", "std::process", "File::open", "TcpStream", "UdpSocket", "Command::new"] {
                if code.contains(banned) {
                    io.push(format!("{file}:{}: {banned}", n + 1));
                }
            }
        }
    }
    assert!(files >= 8, "read {files} source files");
    assert!(fns.len() >= 15, "found {} public functions: the scan matched too little", fns.len());
    for must in ["open", "render_page", "render_size", "prescan"] {
        assert!(fns.iter().any(|(_, name, _)| name == must), "the scan finds `{must}`");
    }
    let banned = ["Path", "OsStr", "OsString", "&str", "String", "Url", "url"];
    let takes_a_path: Vec<String> = fns
        .iter()
        .filter(|(_, _, params)| banned.iter().any(|b| params.contains(b)))
        .map(|(file, name, params)| format!("{file}: {name}({params})"))
        .collect();
    assert!(takes_a_path.is_empty(), "public functions taking a path, a URL or a string: {takes_a_path:?}");
    assert!(io.is_empty(), "file, network or process use in src/: {io:?}");
}
