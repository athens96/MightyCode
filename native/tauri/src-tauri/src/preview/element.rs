//! MC-7: what the preview reads of a picked element and the page's errors, as the words an agent gets.
//!
//! - [`PICK_SCRIPT`] runs on the picked node in the previewed page (DevTools `Runtime.callFunctionOn`, never a script the page can
//!   call): its tag, a selector that finds it alone when one can be made, its main computed styles, some of its text, a few
//!   attributes (never a field's value), its box, and the component it belongs to when the page's framework leaves a mark (React's
//!   fiber, Vue's component, Svelte's dev location, Angular's dev `ng`).
//! - [`describe`] turns that into the attachment's text; [`console_text`] and [`requests_text`] the page's console errors and failed
//!   requests since it last loaded. Everything is cut to size and stripped of control characters: the page is not trusted to be short.

use mighty_delegation::verify::{PageFindings, cap_chars, strip_ansi};
use serde::Deserialize;

/// The longest attachment text.
pub const MAXIMUM_TEXT: usize = 8_000;
/// The padding around an element's screenshot, in CSS pixels.
const SHOT_PADDING: f64 = 8.0;
/// The largest side of an element's screenshot, in CSS pixels.
const SHOT_SIDE: f64 = 4_000.0;

/// Runs with `this` the picked node; answers a JSON string ([`ElementPick`]).
pub const PICK_SCRIPT: &str = r#"function () {
  const cap = (value, n) => { const s = String(value == null ? '' : value).replace(/\s+/g, ' ').trim(); return s.length > n ? s.slice(0, n - 1) + '\u2026' : s; };
  const el = this && this.nodeType === 1 ? this : (this && this.parentElement) || null;
  if (!el) return JSON.stringify({ error: 'not an element' });
  const doc = el.ownerDocument;
  const view = doc.defaultView || window;
  const esc = (v) => (view.CSS && view.CSS.escape ? view.CSS.escape(v) : String(v).replace(/[^a-zA-Z0-9_-]/g, (c) => '\\' + c));
  const unique = (sel) => { try { return doc.querySelectorAll(sel).length === 1; } catch (e) { return false; } };
  const stableClass = (c) => /^[A-Za-z][\w-]{0,40}$/.test(c) && !/\d{3,}|^css-|^sc-|^jsx-|^svelte-|__[A-Za-z0-9]{5,}$/.test(c);
  const testAttributes = ['data-testid', 'data-test', 'data-cy', 'data-qa'];
  const step = (node) => {
    for (const a of testAttributes) {
      const v = node.getAttribute(a);
      if (v) { const s = '[' + a + '="' + v.replace(/["\\]/g, '\\$&') + '"]'; return { s, alone: unique(s) }; }
    }
    if (node.id && !/\d{4,}|:/.test(node.id)) { const s = '#' + esc(node.id); return { s, alone: unique(s) }; }
    let s = node.tagName.toLowerCase();
    const classes = Array.from(node.classList).filter(stableClass).slice(0, 2);
    if (classes.length) s += classes.map((c) => '.' + esc(c)).join('');
    const parent = node.parentElement;
    if (parent) {
      const same = Array.from(parent.children).filter((c) => c.tagName === node.tagName);
      if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
    }
    return { s, alone: false };
  };
  const parts = [];
  let node = el;
  let selector = '';
  while (node && node.nodeType === 1 && parts.length < 8) {
    const here = step(node);
    parts.unshift(here.s);
    selector = parts.join(' > ');
    if (here.alone || unique(selector) || node === doc.documentElement) break;
    node = node.parentElement;
  }
  const secret = el.tagName === 'INPUT' && /password/i.test(el.getAttribute('type') || '');
  const names = ['role', 'aria-label', 'name', 'type', 'href', 'src', 'alt', 'title', 'placeholder', 'for', 'data-testid'];
  const attributes = [];
  for (const n of names) { const v = el.getAttribute(n); if (v != null && v !== '') attributes.push([n, cap(v, 160)]); }
  const classList = Array.from(el.classList).slice(0, 12).join(' ');
  if (classList) attributes.unshift(['class', cap(classList, 200)]);
  if (el.id) attributes.unshift(['id', cap(el.id, 120)]);
  const cs = view.getComputedStyle(el);
  const props = ['display', 'position', 'box-sizing', 'width', 'height', 'margin', 'padding', 'gap', 'border', 'border-radius', 'color',
    'background-color', 'background-image', 'font-family', 'font-size', 'font-weight', 'line-height', 'letter-spacing', 'text-align',
    'text-transform', 'flex-direction', 'flex-wrap', 'justify-content', 'align-items', 'grid-template-columns', 'opacity', 'z-index',
    'overflow', 'box-shadow', 'transform', 'cursor'];
  const quiet = ['none', 'normal', 'auto', '0px', 'rgba(0, 0, 0, 0)', 'visible', 'static', 'start', 'nowrap', 'row', 'stretch'];
  const styles = [];
  for (const p of props) {
    const v = cs.getPropertyValue(p);
    if (!v) continue;
    if (p !== 'display' && p !== 'width' && p !== 'height' && (quiet.includes(v) || (p === 'opacity' && v === '1') || /^0px none/.test(v))) continue;
    styles.push([p, cap(v, 160)]);
  }
  const hints = [];
  let source = null;
  const near = (test) => { let n = el; for (let i = 0; n && i < 8; i++) { const k = test(n); if (k) return [n, k]; n = n.parentElement; } return null; };
  try {
    const found = near((n) => Object.keys(n).find((k) => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$')));
    if (found) {
      let f = found[0][found[1]];
      const list = [];
      for (let guard = 0; f && guard < 300 && list.length < 8; guard++) {
        const t = f.type;
        let name = null;
        if (typeof t === 'function') name = t.displayName || t.name;
        else if (t && typeof t === 'object') name = t.displayName || (t.render && (t.render.displayName || t.render.name)) || (t.type && (t.type.displayName || t.type.name));
        if (name && !/^(Anonymous|_c\d*)$/.test(name) && list[list.length - 1] !== name) list.push(cap(name, 80));
        if (!source && f._debugSource && f._debugSource.fileName) source = f._debugSource.fileName + ':' + f._debugSource.lineNumber;
        f = f.return;
      }
      if (list.length) hints.push({ framework: 'React', components: list });
    }
  } catch (e) {}
  try {
    const found = near((n) => (n.__vueParentComponent ? '__vueParentComponent' : null));
    if (found) {
      let c = found[0].__vueParentComponent;
      const list = [];
      for (let guard = 0; c && guard < 60 && list.length < 8; guard++) {
        const t = c.type || {};
        const name = t.name || t.__name || (t.__file ? String(t.__file).split('/').pop().replace(/\.vue$/, '') : null);
        if (name && list[list.length - 1] !== name) list.push(cap(name, 80));
        if (!source && t.__file) source = String(t.__file);
        c = c.parent;
      }
      if (list.length) hints.push({ framework: 'Vue', components: list });
    }
  } catch (e) {}
  try {
    const found = near((n) => (n.__svelte_meta && n.__svelte_meta.loc ? '__svelte_meta' : null));
    if (found) {
      const loc = found[0].__svelte_meta.loc;
      if (!source) source = loc.file + ':' + (Number(loc.line) + 1);
      hints.push({ framework: 'Svelte', components: [cap(String(loc.file).split('/').pop(), 80)] });
    }
  } catch (e) {}
  try {
    if (view.ng && view.ng.getOwningComponent) {
      const c = view.ng.getOwningComponent(el);
      if (c && c.constructor && c.constructor.name) hints.push({ framework: 'Angular', components: [cap(c.constructor.name, 80)] });
    }
  } catch (e) {}
  const r = el.getBoundingClientRect();
  const html = el.outerHTML || '';
  const open = html.indexOf('>');
  return JSON.stringify({
    tag: el.tagName.toLowerCase(),
    selector,
    selectorUnique: unique(selector),
    html: secret ? '' : cap(open > 0 ? html.slice(0, open + 1) : html, 300),
    text: secret ? '' : cap(el.innerText || el.textContent || '', 300),
    attributes,
    styles,
    components: hints,
    source: source ? cap(source, 300) : null,
    rect: { x: r.left + view.scrollX, y: r.top + view.scrollY, width: r.width, height: r.height },
    page: cap(view.location.href, 500),
    title: cap(doc.title, 200),
    viewport: { width: view.innerWidth, height: view.innerHeight },
  });
}"#;

/// Runs with `this` the picked node: its box in the page now, `[x, y, width, height]` (CSS pixels from the page's top left).
pub const RECT_SCRIPT: &str = r#"function () {
  const el = this && this.nodeType === 1 ? this : (this && this.parentElement) || null;
  if (!el || !el.isConnected) return null;
  const view = el.ownerDocument.defaultView || window;
  const r = el.getBoundingClientRect();
  return [r.left + view.scrollX, r.top + view.scrollY, r.width, r.height];
}"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ComponentHint {
    pub framework: String,
    /// Innermost first.
    pub components: Vec<String>,
}

/// What [`PICK_SCRIPT`] read of the element.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ElementPick {
    pub tag: String,
    pub selector: String,
    pub selector_unique: bool,
    pub html: String,
    pub text: String,
    pub attributes: Vec<(String, String)>,
    pub styles: Vec<(String, String)>,
    pub components: Vec<ComponentHint>,
    pub source: Option<String>,
    pub rect: Rect,
    pub page: String,
    pub title: String,
    pub viewport: Viewport,
    pub error: Option<String>,
}

/// `text` on one line without control characters, at most `max` characters.
fn clean(text: &str, max: usize) -> String {
    let plain: String = strip_ansi(text).chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cap_chars(plain.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), max)
}

/// A tag fit for a file name (letters and digits, else `element`).
fn tag_of(pick: &ElementPick) -> String {
    let tag: String = pick.tag.chars().filter(char::is_ascii_alphanumeric).take(20).collect();
    if tag.is_empty() { "element".into() } else { tag.to_ascii_lowercase() }
}

/// The attachment's name for a picked element's description.
pub fn file_name(pick: &ElementPick) -> String {
    format!("preview-element-{}.txt", tag_of(pick))
}

/// The attachment's name for a picked element's screenshot (`ext` png or jpg).
pub fn shot_name(tag: Option<&str>, ext: &str) -> String {
    match tag.map(|t| t.chars().filter(char::is_ascii_alphanumeric).take(20).collect::<String>()).filter(|t| !t.is_empty()) {
        Some(tag) => format!("preview-{}.{ext}", tag.to_ascii_lowercase()),
        None => format!("preview-page.{ext}"),
    }
}

/// The words an agent gets about a picked element.
pub fn describe(pick: &ElementPick) -> String {
    let mut out = String::from("Element picked in the live preview (Mighty Code)\n");
    if !pick.page.is_empty() {
        out.push_str(&format!("Page: {}\n", clean(&pick.page, 500)));
    }
    if !pick.title.is_empty() {
        out.push_str(&format!("Page title: {}\n", clean(&pick.title, 200)));
    }
    out.push_str(&format!("Element: <{}>\n", clean(&pick.tag, 40)));
    if !pick.selector.is_empty() {
        let alone = if pick.selector_unique { "" } else { " (not unique on the page)" };
        out.push_str(&format!("Selector: {}{alone}\n", clean(&pick.selector, 600)));
    }
    for hint in &pick.components {
        let mut names: Vec<String> = hint.components.iter().take(8).map(|n| clean(n, 80)).filter(|n| !n.is_empty()).collect();
        if names.is_empty() {
            continue;
        }
        names.reverse();
        out.push_str(&format!("Component ({}): {}\n", clean(&hint.framework, 20), names.join(" > ")));
    }
    if let Some(source) = pick.source.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("Source: {}\n", clean(source, 300)));
    }
    if !pick.text.trim().is_empty() {
        out.push_str(&format!("Text: \"{}\"\n", clean(&pick.text, 300)));
    }
    if !pick.html.is_empty() {
        out.push_str(&format!("HTML: {}\n", clean(&pick.html, 300)));
    }
    if !pick.attributes.is_empty() {
        let list: Vec<String> =
            pick.attributes.iter().take(16).map(|(name, value)| format!("{}=\"{}\"", clean(name, 40), clean(value, 160))).collect();
        out.push_str(&format!("Attributes: {}\n", list.join(" ")));
    }
    let r = pick.rect;
    out.push_str(&format!(
        "Box: {} x {} at ({}, {}) in a {} x {} viewport\n",
        r.width.round(),
        r.height.round(),
        r.x.round(),
        r.y.round(),
        pick.viewport.width.round(),
        pick.viewport.height.round()
    ));
    if !pick.styles.is_empty() {
        out.push_str("Computed styles:\n");
        for (name, value) in pick.styles.iter().take(40) {
            out.push_str(&format!("  {}: {}\n", clean(name, 40), clean(value, 160)));
        }
    }
    cap_chars(&out, MAXIMUM_TEXT)
}

/// The screenshot clip of a box: padded, inside the page, at most [`SHOT_SIDE`] a side; None for a box with no size.
pub fn clip(x: f64, y: f64, width: f64, height: f64) -> Option<(f64, f64, f64, f64)> {
    if !(x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite()) || width < 1.0 || height < 1.0 {
        return None;
    }
    let left = (x - SHOT_PADDING).max(0.0);
    let top = (y - SHOT_PADDING).max(0.0);
    let w = (x + width + SHOT_PADDING - left).min(SHOT_SIDE);
    let h = (y + height + SHOT_PADDING - top).min(SHOT_SIDE);
    Some((left.floor(), top.floor(), w.ceil(), h.ceil()))
}

fn listed(title: &str, url: &str, lines: &[String]) -> String {
    let mut out = format!("{title} (Mighty Code live preview)\nPage: {}\n", clean(url, 500));
    for line in lines {
        out.push_str(&format!("- {}\n", clean(line, 1_000)));
    }
    cap_chars(&out, MAXIMUM_TEXT)
}

/// The page's console errors and uncaught exceptions since it last loaded; None when there are none.
pub fn console_text(findings: &PageFindings) -> Option<String> {
    let lines: Vec<String> = findings
        .console_errors
        .iter()
        .map(|e| format!("console.error: {e}"))
        .chain(findings.exceptions.iter().map(|e| format!("uncaught: {e}")))
        .collect();
    (!lines.is_empty()).then(|| listed("Console errors", &findings.url, &lines))
}

/// The page's failed requests (4xx/5xx or a network error) since it last loaded; None when there are none.
pub fn requests_text(findings: &PageFindings) -> Option<String> {
    (!findings.failed_requests.is_empty()).then(|| listed("Failed requests", &findings.url, &findings.failed_requests))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick() -> ElementPick {
        serde_json::from_str(
            r##"{"tag":"button","selector":"main > form > button.primary","selectorUnique":true,"html":"<button class=\"primary\" type=\"submit\">",
            "text":"  Save\n changes ","attributes":[["class","primary"],["type","submit"]],"styles":[["display","inline-flex"],["padding","4px 12px"]],
            "components":[{"framework":"React","components":["SaveButton","SettingsForm","App"]}],"source":"src/SaveButton.tsx:12",
            "rect":{"x":480.4,"y":312,"width":120,"height":36},"page":"http://localhost:41000/settings","title":"Settings",
            "viewport":{"width":1280,"height":800}}"##,
        )
        .unwrap()
    }

    #[test]
    fn a_picked_element_is_described_for_the_agent() {
        let text = describe(&pick());
        assert!(text.contains("Page: http://localhost:41000/settings"), "{text}");
        assert!(text.contains("Element: <button>"));
        assert!(text.contains("Selector: main > form > button.primary\n"), "a unique selector says nothing more");
        assert!(text.contains("Component (React): App > SettingsForm > SaveButton"), "outermost first: {text}");
        assert!(text.contains("Source: src/SaveButton.tsx:12"));
        assert!(text.contains("Text: \"Save changes\""), "the text on one line");
        assert!(text.contains("Attributes: class=\"primary\" type=\"submit\""));
        assert!(text.contains("Box: 120 x 36 at (480, 312) in a 1280 x 800 viewport"));
        assert!(text.contains("  padding: 4px 12px"));
        assert_eq!(file_name(&pick()), "preview-element-button.txt");
    }

    #[test]
    fn what_the_page_says_is_cut_and_never_breaks_a_line() {
        let mut long = pick();
        long.text = format!("a\u{1b}[31mred\u{7}\r\n{}", "x".repeat(20_000));
        long.selector = "div".into();
        long.selector_unique = false;
        long.tag = "../evil tag".into();
        long.styles = (0..500).map(|i| (format!("p{i}"), "v".repeat(500))).collect();
        let text = describe(&long);
        assert!(text.chars().count() <= MAXIMUM_TEXT);
        assert!(text.contains("Selector: div (not unique on the page)"));
        assert!(!text.contains('\u{1b}') && !text.contains('\u{7}') && !text.contains('\r'));
        assert!(text.lines().any(|l| l.starts_with("Text: \"ared ")), "{text}");
        assert_eq!(file_name(&long), "preview-element-eviltag.txt", "a file name, never a folder");
    }

    #[test]
    fn an_elements_screenshot_is_padded_and_kept_inside_the_page() {
        assert_eq!(clip(100.0, 50.0, 20.0, 10.0), Some((92.0, 42.0, 36.0, 26.0)));
        assert_eq!(clip(2.0, 3.0, 10.0, 10.0), Some((0.0, 0.0, 20.0, 21.0)), "never above or left of the page");
        assert_eq!(clip(0.0, 0.0, 9_000.0, 20_000.0), Some((0.0, 0.0, 4_000.0, 4_000.0)));
        assert_eq!(clip(0.0, 0.0, 0.0, 10.0), None);
        assert_eq!(clip(f64::NAN, 0.0, 10.0, 10.0), None);
        assert_eq!(shot_name(Some("button"), "png"), "preview-button.png");
        assert_eq!(shot_name(Some("../"), "jpg"), "preview-page.jpg");
        assert_eq!(shot_name(None, "png"), "preview-page.png");
    }

    #[test]
    fn the_pages_errors_are_listed_only_when_there_are_some() {
        let mut findings = PageFindings { url: "http://localhost:41000/".into(), ..PageFindings::default() };
        assert_eq!(console_text(&findings), None);
        assert_eq!(requests_text(&findings), None);
        findings.console_errors.push("boom".into());
        findings.exceptions.push("TypeError: x is undefined".into());
        findings.failed_requests.push("404 GET http://localhost:41000/api/user".into());
        let console = console_text(&findings).unwrap();
        assert!(console.contains("- console.error: boom\n- uncaught: TypeError: x is undefined"), "{console}");
        assert!(requests_text(&findings).unwrap().contains("- 404 GET http://localhost:41000/api/user"));
    }

    #[test]
    fn the_pick_script_reaches_nothing_but_the_page() {
        // It is DevTools that runs it on the node; it names no bridge, binding or message channel back to the app.
        for script in [PICK_SCRIPT, RECT_SCRIPT] {
            for word in ["__TAURI", "ipc", "postMessage", "chrome.webview", "fetch(", "XMLHttpRequest", "WebSocket", "binding"] {
                assert!(!script.contains(word), "{word}");
            }
        }
        // A password field's text and markup are never read.
        assert!(PICK_SCRIPT.contains("const secret = el.tagName === 'INPUT' && /password/i"));
        assert!(!PICK_SCRIPT.contains("'value'"), "no field's value is read");
    }
}
