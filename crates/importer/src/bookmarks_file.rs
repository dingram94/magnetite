//! A bookmarks file: the HTML that Safari, Chrome, Firefox and most other
//! browsers write with "Export Bookmarks" (the Netscape bookmark format,
//! which `export.rs` writes too). It is a list of `<DT><A HREF=…>title</A>`
//! lines; a folder is `<DT><H3>name</H3>` followed by its own `<DL>` list.
//! Browsers write it loosely, so this reads tags rather than parsing HTML.

use magnetite_core::{Folder, SidebarNode, TabItem};

use crate::{Ids, ImportError, MAX_DEPTH, importable};

/// The bookmarks and folders of the file `html`, nested as they are there.
/// Folders that end up empty and addresses that are no web pages are left
/// out. A file without any bookmark is `ImportError::Empty`.
pub fn bookmarks(html: &str) -> Result<Vec<SidebarNode>, ImportError> {
    let mut ids = Ids::new("file");
    // The lists that are open, innermost last, each with its folder's name
    // (the first is the file's own list).
    let mut open: Vec<(String, Vec<SidebarNode>)> = vec![(String::new(), Vec::new())];
    // The name of a folder whose list has not started yet.
    let mut named: Option<String> = None;
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let (tag, after) = (&rest[..end], rest.get(end + 1..).unwrap_or(""));
        let name = tag
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "h3" => named = Some(text_until(after, "</h3")),
            "dl" => {
                if open.len() > MAX_DEPTH {
                    return Err(ImportError::TooDeep(MAX_DEPTH));
                }
                // The file's own list has no folder line before it.
                if let Some(title) = named.take() {
                    open.push((title, Vec::new()));
                }
            }
            "/dl" if open.len() > 1 => {
                let (title, children) = open.pop().unwrap_or_default();
                if !children.is_empty() {
                    let folder = Folder {
                        id: ids.next()?,
                        title: if title.is_empty() {
                            "Untitled".into()
                        } else {
                            title
                        },
                        icon: None,
                        children,
                        smart: None,
                    };
                    last(&mut open).push(SidebarNode::Folder(folder));
                }
            }
            "a" => {
                let title = text_until(after, "</a");
                if let Some(url) = attribute(tag, "href").filter(|u| importable(u)) {
                    let added = attribute(tag, "add_date").and_then(|d| d.parse().ok());
                    last(&mut open).push(SidebarNode::Tab(TabItem {
                        custom_title: !title.is_empty(),
                        created_at: added.filter(|&t: &i64| t > 0),
                        ..TabItem::new(
                            ids.next()?,
                            &url,
                            if title.is_empty() { &url } else { &title },
                        )
                    }));
                }
            }
            _ => {}
        }
        rest = after;
    }
    // A file that ends inside folders: what they hold goes up a level.
    while open.len() > 1 {
        let (_, children) = open.pop().unwrap_or_default();
        last(&mut open).extend(children);
    }
    let found = open.pop().map(|(_, list)| list).unwrap_or_default();
    if found.is_empty() {
        return Err(ImportError::Empty("The file".into()));
    }
    Ok(found)
}

/// The bookmarks of a file in any format Magnetite writes
/// (`export.rs`): the HTML bookmarks file, or its own JSON or CSV. The
/// format is told by how the text starts. A JSON or CSV file of several
/// spaces gives a folder per space, as the HTML file of several spaces
/// does; one space gives its bookmarks as they are.
pub fn bookmarks_any(text: &str) -> Result<Vec<SidebarNode>, ImportError> {
    let start = text.trim_start_matches('\u{feff}').trim_start();
    if start.starts_with('[') {
        json_bookmarks(start)
    } else if start
        .lines()
        .next()
        .is_some_and(|line| csv_line(line) == CSV_HEAD)
    {
        csv_bookmarks(start)
    } else {
        bookmarks(text)
    }
}

/// The columns of the CSV `export::space_bookmarks_csv` writes.
const CSV_HEAD: [&str; 4] = ["space", "folder", "title", "url"];

/// One space of a JSON or CSV file as a folder; a file of one space as
/// that space's bookmarks. Nothing found is `ImportError::Empty`.
fn by_space(
    spaces: Vec<(String, Vec<SidebarNode>)>,
    ids: &mut Ids,
) -> Result<Vec<SidebarNode>, ImportError> {
    let mut spaces: Vec<_> = (spaces.into_iter())
        .filter(|(_, list)| !list.is_empty())
        .collect();
    let found = if spaces.len() == 1 {
        spaces.remove(0).1
    } else {
        let mut folders = Vec::new();
        for (title, children) in spaces {
            folders.push(SidebarNode::Folder(folder(ids.next()?, title, children)));
        }
        folders
    };
    if found.is_empty() {
        return Err(ImportError::Empty("The file".into()));
    }
    Ok(found)
}

fn folder(id: String, title: String, children: Vec<SidebarNode>) -> Folder {
    Folder {
        id,
        title: if title.trim().is_empty() {
            "Untitled".into()
        } else {
            title
        },
        icon: None,
        children,
        smart: None,
    }
}

fn bookmark(id: String, url: &str, title: &str) -> SidebarNode {
    SidebarNode::Tab(TabItem {
        custom_title: !title.is_empty(),
        ..TabItem::new(id, url, if title.is_empty() { url } else { title })
    })
}

/// Magnetite's JSON: a list of `{"space", "bookmarks"}`; a bookmark is
/// `{"title", "url"}`, a folder `{"folder", "children"}`.
fn json_bookmarks(text: &str) -> Result<Vec<SidebarNode>, ImportError> {
    fn tree(
        list: &serde_json::Value,
        depth: usize,
        ids: &mut Ids,
    ) -> Result<Vec<SidebarNode>, ImportError> {
        if depth > MAX_DEPTH {
            return Err(ImportError::TooDeep(MAX_DEPTH));
        }
        let mut out = Vec::new();
        for node in list.as_array().map(Vec::as_slice).unwrap_or_default() {
            let text = |key: &str| node.get(key).and_then(serde_json::Value::as_str);
            if let Some(children) = node.get("children") {
                let children = tree(children, depth + 1, ids)?;
                if !children.is_empty() {
                    let title = text("folder").unwrap_or_default().to_string();
                    out.push(SidebarNode::Folder(folder(ids.next()?, title, children)));
                }
            } else if let Some(url) = text("url").filter(|u| importable(u)) {
                out.push(bookmark(
                    ids.next()?,
                    url,
                    text("title").unwrap_or_default(),
                ));
            }
        }
        Ok(out)
    }
    let file: serde_json::Value = serde_json::from_str(text).map_err(|e| ImportError::Corrupt {
        what: "that file".into(),
        detail: e.to_string(),
    })?;
    let mut ids = Ids::new("file");
    let mut spaces = Vec::new();
    for space in file.as_array().map(Vec::as_slice).unwrap_or_default() {
        let title = space.get("space").and_then(serde_json::Value::as_str);
        let list = space.get("bookmarks").unwrap_or(&serde_json::Value::Null);
        spaces.push((
            title.unwrap_or_default().to_string(),
            tree(list, 1, &mut ids)?,
        ));
    }
    by_space(spaces, &mut ids)
}

/// Magnetite's CSV: a line per bookmark, `space,folder,title,url`, the
/// folder as its path with " / " between. A title with the `'` the export
/// puts before what a spreadsheet would take for a formula loses it.
fn csv_bookmarks(text: &str) -> Result<Vec<SidebarNode>, ImportError> {
    /// The folder at `path` below `list`, made on the way where it is missing.
    fn place<'a>(
        list: &'a mut Vec<SidebarNode>,
        path: &[&str],
        ids: &mut Ids,
    ) -> Result<&'a mut Vec<SidebarNode>, ImportError> {
        let Some((name, rest)) = path.split_first() else {
            return Ok(list);
        };
        let named = |n: &SidebarNode| matches!(n, SidebarNode::Folder(f) if f.title == *name);
        let at = if let Some(at) = list.iter().position(named) {
            at
        } else {
            let made = folder(ids.next()?, (*name).to_string(), Vec::new());
            list.push(SidebarNode::Folder(made));
            list.len() - 1
        };
        // `at` is a folder: found as one, or just made.
        match list.get_mut(at) {
            Some(SidebarNode::Folder(f)) => place(&mut f.children, rest, ids),
            _ => Err(ImportError::Empty("The file".into())),
        }
    }
    let mut ids = Ids::new("file");
    let mut spaces: Vec<(String, Vec<SidebarNode>)> = Vec::new();
    for row in csv_rows(text).into_iter().skip(1) {
        let [space, path, title, url] = row.as_slice() else {
            continue;
        };
        if !importable(url) {
            continue;
        }
        let (space, title) = (unguarded(space), unguarded(title));
        let path = path_steps(unguarded(path));
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        if path.len() >= MAX_DEPTH {
            return Err(ImportError::TooDeep(MAX_DEPTH));
        }
        let at = if let Some(at) = spaces.iter().position(|(name, _)| name == space) {
            at
        } else {
            spaces.push((space.to_string(), Vec::new()));
            spaces.len() - 1
        };
        let id = ids.next()?;
        place(&mut spaces[at].1, &path, &mut ids)?.push(bookmark(id, url, title));
    }
    by_space(spaces, &mut ids)
}

/// A CSV field without the `'` the export puts before what a spreadsheet
/// would take for a formula, and before a `'` of the field's own.
fn unguarded(field: &str) -> &str {
    let rest = field.strip_prefix('\'');
    rest.filter(|r| r.starts_with(['=', '+', '-', '@', '\'']))
        .unwrap_or(field)
}

/// The folders of a path as the export writes it: steps parted by `/`
/// (with blanks around it), a `/` or `\` of a name behind a `\`.
fn path_steps(path: &str) -> Vec<String> {
    let mut steps = vec![String::new()];
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => steps
                .last_mut()
                .into_iter()
                .for_each(|s| s.extend(chars.next())),
            '/' => steps.push(String::new()),
            c => steps.last_mut().into_iter().for_each(|s| s.push(c)),
        }
    }
    let steps = steps.iter().map(|s| s.trim().to_string());
    steps.filter(|s| !s.is_empty()).collect()
}

/// The fields of one CSV line without line breaks inside fields: enough to
/// know the header.
fn csv_line(line: &str) -> Vec<String> {
    csv_rows(line).into_iter().next().unwrap_or_default()
}

/// The rows of a CSV text, each as its fields: commas between, a field in
/// double quotes may hold commas, line breaks and `""` for a quote.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let (mut row, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.is_empty()));
    rows
}

/// The innermost open list.
fn last(open: &mut [(String, Vec<SidebarNode>)]) -> &mut Vec<SidebarNode> {
    let at = open.len() - 1;
    &mut open[at].1
}

/// The text from the start of `html` up to the closing tag `close`
/// (lower case, without its `>`), without tags inside it and with its
/// character references resolved.
fn text_until(html: &str, close: &str) -> String {
    let end = html.to_ascii_lowercase().find(close).unwrap_or(html.len());
    let mut text = String::new();
    let mut in_tag = false;
    for c in html[..end].chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    unescape(text.trim())
}

/// The value of attribute `name` (lower case) in the inside of a tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let at = from + lower[from..].find(name)?;
        from = at + name.len();
        // A whole attribute name, not the end of another one.
        let starts = tag[..at].ends_with(char::is_whitespace);
        let value = tag[from..].trim_start();
        let Some(value) = value.strip_prefix('=').filter(|_| starts) else {
            continue;
        };
        let value = value.trim_start();
        let quoted = value.chars().next().filter(|q| matches!(q, '"' | '\''));
        return Some(unescape(match quoted {
            Some(q) => value[1..].split(q).next().unwrap_or(""),
            None => value.split_whitespace().next().unwrap_or(""),
        }));
    }
}

/// `text` with the character references bookmark files use resolved.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let known = rest[1..].find(';').filter(|&end| end <= 8).and_then(|end| {
            let name = &rest[1..=end];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => ' ',
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end + 2))
        });
        let (c, len) = known.unwrap_or(('&', 1));
        out.push(c);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}
