//! Output formats other than our own JSON.

use std::fmt::Write;

use magnetite_core::{SidebarNode, SidebarSnapshot, Space, TabItem};

/// Netscape bookmarks HTML, importable by every browser. Favorites become a
/// top-level folder, each space a folder with "Pinned", "Today" and
/// "Bookmarks" below.
pub fn bookmarks_html(snapshot: &SidebarSnapshot) -> String {
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    for favs in &snapshot.favorites {
        if favs.items.is_empty() {
            continue;
        }
        folder_open(&mut out, 1, "Favorites");
        for t in &favs.items {
            link(&mut out, 2, t);
        }
        folder_close(&mut out, 1);
    }
    for space in &snapshot.spaces {
        folder_open(&mut out, 1, &space.title);
        if !space.pinned.is_empty() {
            folder_open(&mut out, 2, "Pinned");
            nodes(&mut out, 3, &space.pinned);
            folder_close(&mut out, 2);
        }
        if !space.today.is_empty() {
            folder_open(&mut out, 2, "Today");
            for t in &space.today {
                link(&mut out, 3, t);
            }
            folder_close(&mut out, 2);
        }
        if !space.bookmarks.is_empty() {
            folder_open(&mut out, 2, "Bookmarks");
            nodes(&mut out, 3, &space.bookmarks);
            folder_close(&mut out, 2);
        }
        folder_close(&mut out, 1);
    }
    out.push_str("</DL><p>\n");
    out
}

/// The bookmarks of `spaces` (what a space has saved, not its tabs) as
/// Netscape bookmarks HTML. With more than one space each is a folder.
pub fn space_bookmarks_html(spaces: &[&Space]) -> String {
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    for space in spaces {
        if spaces.len() > 1 {
            folder_open(&mut out, 1, &space.title);
            nodes(&mut out, 2, &space.bookmarks);
            folder_close(&mut out, 1);
        } else {
            nodes(&mut out, 1, &space.bookmarks);
        }
    }
    out.push_str("</DL><p>\n");
    out
}

/// The bookmarks of `spaces` as JSON: a list of spaces, each with its name
/// and its bookmarks; a bookmark is `{"title", "url"}`, a folder
/// `{"folder", "children"}`.
pub fn space_bookmarks_json(spaces: &[&Space]) -> String {
    fn tree(list: &[SidebarNode]) -> Vec<serde_json::Value> {
        list.iter()
            .filter_map(|node| match node {
                SidebarNode::Tab(t) => crate::importable(&t.url)
                    .then(|| serde_json::json!({ "title": plain(&t.title), "url": t.url })),
                SidebarNode::Folder(f) => Some(
                    serde_json::json!({ "folder": plain(&f.title), "children": tree(&f.children) }),
                ),
            })
            .collect()
    }
    let all: Vec<serde_json::Value> = spaces
        .iter()
        .map(|s| serde_json::json!({ "space": plain(&s.title), "bookmarks": tree(&s.bookmarks) }))
        .collect();
    let mut out = serde_json::to_string_pretty(&all).unwrap_or_else(|_| "[]".into());
    out.push('\n');
    out
}

/// The bookmarks of `spaces` as CSV, a line each: `space,folder,title,url`,
/// the folder as its path with " / " between (a `/` or `\` inside a name
/// has a `\` before it). Every field is quoted. A field that a spreadsheet
/// would take for a formula (it starts with `=`, `+`, `-` or `@`) gets a
/// `'` in front, titles come from web pages; and so does one that starts
/// with `'` itself, which keeps the guard reversible.
pub fn space_bookmarks_csv(spaces: &[&Space]) -> String {
    fn field(text: &str) -> String {
        let text = plain(text);
        // Also before a `'` of its own, so the import can take the guard
        // off again without taking a name's own apostrophe.
        let guard = if text.starts_with(['=', '+', '-', '@', '\'']) {
            "'"
        } else {
            ""
        };
        format!("\"{guard}{}\"", text.replace('"', "\"\""))
    }
    fn rows(out: &mut String, space: &str, path: &[String], list: &[SidebarNode]) {
        for node in list {
            match node {
                SidebarNode::Tab(t) if crate::importable(&t.url) => {
                    let _ = write!(
                        out,
                        "{},{},{},{}\r\n",
                        field(space),
                        field(&path.join(" / ")),
                        field(&t.title),
                        field(&t.url)
                    );
                }
                SidebarNode::Tab(_) => {}
                SidebarNode::Folder(f) => {
                    let mut below = path.to_vec();
                    // A `/` in a name is no step of the path.
                    below.push(plain(&f.title).replace('\\', "\\\\").replace('/', "\\/"));
                    rows(out, space, &below, &f.children);
                }
            }
        }
    }
    let mut out = String::from("space,folder,title,url\r\n");
    for space in spaces {
        rows(&mut out, &space.title, &[], &space.bookmarks);
    }
    out
}

/// `s` without control characters.
fn plain(s: &str) -> String {
    s.replace(|c: char| c.is_control(), "")
}

fn nodes(out: &mut String, depth: usize, list: &[SidebarNode]) {
    for node in list {
        match node {
            SidebarNode::Tab(t) => link(out, depth, t),
            SidebarNode::Folder(f) => {
                folder_open(out, depth, &f.title);
                nodes(out, depth + 1, &f.children);
                folder_close(out, depth);
            }
        }
    }
}

fn folder_open(out: &mut String, depth: usize, title: &str) {
    let pad = "    ".repeat(depth);
    let _ = writeln!(out, "{pad}<DT><H3>{}</H3>", escape(title));
    let _ = writeln!(out, "{pad}<DL><p>");
}

fn folder_close(out: &mut String, depth: usize) {
    let _ = writeln!(out, "{}</DL><p>", "    ".repeat(depth));
}

fn link(out: &mut String, depth: usize, tab: &TabItem) {
    // Only web addresses, as the in-app import keeps them: no `javascript:`
    // or `file:` link in a file meant for another browser.
    if !crate::importable(&tab.url) {
        return;
    }
    let pad = "    ".repeat(depth);
    let added = tab
        .created_at
        .map(|t| format!(" ADD_DATE=\"{t}\""))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "{pad}<DT><A HREF=\"{}\"{added}>{}</A>",
        escape(&tab.url),
        escape(&tab.title)
    );
}

/// HTML-escaped, without control characters (the export may go to a
/// terminal).
fn escape(s: &str) -> String {
    s.replace(|c: char| c.is_control(), "")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use magnetite_core::Folder;

    fn tab(title: &str, url: &str) -> SidebarNode {
        SidebarNode::Tab(TabItem {
            title: title.into(),
            url: url.into(),
            ..TabItem::default()
        })
    }

    fn space(title: &str, bookmarks: Vec<SidebarNode>) -> Space {
        Space {
            id: title.into(),
            title: title.into(),
            icon: None,
            icon_image: None,
            accent: None,
            profile_id: "p".into(),
            shortcut: None,
            pinned: vec![tab("pinned, not a bookmark", "https://pinned.example/")],
            today: vec![],
            bookmarks,
            other: magnetite_core::Other::new(),
        }
    }

    fn two() -> (Space, Space) {
        let work = space(
            "Work",
            vec![
                tab("Docs", "https://docs.example/"),
                SidebarNode::Folder(Folder {
                    id: "f".into(),
                    title: "Read \"later\"".into(),
                    icon: None,
                    children: vec![
                        tab("=1+1", "https://a.example/?q=1,2"),
                        tab("script", "javascript:alert(1)"),
                    ],
                    smart: None,
                    other: magnetite_core::Other::new(),
                }),
            ],
        );
        let home = space("Home", vec![tab("News", "https://news.example/")]);
        (work, home)
    }

    /// The titles and addresses of a tree, folders in brackets.
    fn shape(nodes: &[SidebarNode]) -> String {
        let one = |n: &SidebarNode| match n {
            SidebarNode::Tab(t) => format!("{}={}", t.title, t.url),
            SidebarNode::Folder(f) => format!("{}[{}]", f.title, shape(&f.children)),
        };
        nodes.iter().map(one).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn every_format_written_is_read_back_the_same() {
        use crate::bookmarks_file::bookmarks_any;
        let (work, home) = two();
        let both = "Work[Docs=https://docs.example/ Read \"later\"[=1+1=https://a.example/?q=1,2]] \
                    Home[News=https://news.example/]";
        let one = "Docs=https://docs.example/ Read \"later\"[=1+1=https://a.example/?q=1,2]";
        for write in [
            space_bookmarks_html,
            space_bookmarks_json,
            space_bookmarks_csv,
        ] {
            let back = bookmarks_any(&write(&[&work, &home])).expect("our own file reads");
            assert_eq!(shape(&back), both);
            // One space alone comes back without a folder around it.
            let back = bookmarks_any(&write(&[&work])).expect("our own file reads");
            assert_eq!(shape(&back), one);
        }
        // Names that look like a path, a formula or a guard come back as they are.
        let odd = space(
            "=Work",
            vec![SidebarNode::Folder(Folder {
                id: "f".into(),
                title: "CI / CD".into(),
                icon: None,
                children: vec![
                    SidebarNode::Folder(Folder {
                        id: "g".into(),
                        title: "-a\\b".into(),
                        icon: None,
                        children: vec![tab("'=1", "https://one.example/")],
                        smart: None,
                        other: magnetite_core::Other::new(),
                    }),
                    tab("'quoted'", "https://two.example/"),
                ],
                smart: None,
                other: magnetite_core::Other::new(),
            })],
        );
        let home = space("Home", vec![tab("News", "https://news.example/")]);
        let back = bookmarks_any(&space_bookmarks_csv(&[&odd, &home])).expect("reads");
        assert_eq!(
            shape(&back),
            "=Work[CI / CD[-a\\b['=1=https://one.example/] 'quoted'=https://two.example/]] \
             Home[News=https://news.example/]"
        );
        // Nothing in it, or not ours: an error, not an empty import.
        assert!(bookmarks_any("[]").is_err());
        assert!(bookmarks_any("space,folder,title,url\r\n").is_err());
        assert!(bookmarks_any("[{\"space\": 1").is_err());
    }

    #[test]
    fn bookmarks_of_spaces_as_html_come_back_through_the_import() {
        let (work, home) = two();
        let html = space_bookmarks_html(&[&work, &home]);
        assert!(html.contains("<H3>Work</H3>") && html.contains("<H3>Home</H3>"));
        assert!(!html.contains("pinned.example") && !html.contains("javascript:"));
        let back = crate::bookmarks_file::bookmarks(&html).expect("our own file reads");
        let urls: Vec<String> = back
            .iter()
            .flat_map(SidebarNode::tabs)
            .map(|t| t.url.clone())
            .collect();
        assert_eq!(
            urls,
            [
                "https://docs.example/",
                "https://a.example/?q=1,2",
                "https://news.example/"
            ]
        );
        // One space: no folder for it.
        assert!(!space_bookmarks_html(&[&home]).contains("<H3>"));
    }

    #[test]
    fn bookmarks_of_spaces_as_json_keep_their_folders() {
        let (work, home) = two();
        let json: serde_json::Value =
            serde_json::from_str(&space_bookmarks_json(&[&work, &home])).expect("JSON");
        assert_eq!(json[0]["space"], "Work");
        assert_eq!(json[0]["bookmarks"][0]["url"], "https://docs.example/");
        assert_eq!(json[0]["bookmarks"][1]["folder"], "Read \"later\"");
        assert_eq!(
            json[0]["bookmarks"][1]["children"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(json[1]["bookmarks"][0]["title"], "News");
    }

    #[test]
    fn bookmarks_of_spaces_as_csv_are_quoted_and_no_formulas() {
        let (work, home) = two();
        let csv = space_bookmarks_csv(&[&work, &home]);
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(lines[0], "space,folder,title,url");
        assert_eq!(lines[1], "\"Work\",\"\",\"Docs\",\"https://docs.example/\"");
        assert_eq!(
            lines[2],
            "\"Work\",\"Read \"\"later\"\"\",\"'=1+1\",\"https://a.example/?q=1,2\""
        );
        assert_eq!(lines[3], "\"Home\",\"\",\"News\",\"https://news.example/\"");
        assert_eq!(
            lines.len(),
            5,
            "no line for the script, an empty one at the end"
        );
    }
}
