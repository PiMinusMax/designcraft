//! Object › Generate QR Code: a vector QR code (a compound path) as a new object, inside the
//! selected frame, or replacing a selected code (Edit QR Code).

use designcraft_doc::{Content, Fill, Item, ItemId, Selection, Shape};
use designcraft_geom::Rect;
use designcraft_geom::qr::{Ecc, Finder, Modules, QrPart, QrStyle};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_doc, rect_param, spread_param, str_param};
use crate::{Result, Session};

/// Alt text prefix that marks a generated code (and carries its content for editing).
const MARK: &str = "QR code: ";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "object.qrCode",
        "Generate QR Code…",
        ["Object"],
        None,
        "{type?: url|text|sms|email|vcard (default url), content (url/text), number?, message?, to?, subject?, body?, name?, phone?, email?, org?, url?, color? (swatch, [Black]: the code; a gradient swatch too), finderColor? (swatch for the three corner patterns; none: the code's), background? (swatch behind the code and its quiet zone; none: transparent), rect? (new object; else into the selected frame, or re-encode a selected code), spread?, ecc?: low|medium|quartile|high (error correction; high with a logo), modules?: square|rounded|dots|connected, roundness? (0–1), moduleSize? (0.5–1 of a cell: below 1 the modules stand apart), finder?: square|rounded|circle (the three corner patterns), logoSize? (0–0.34 of the side, left clear in the middle), logo? (an image file placed there, fitted; logoSize 0.25 and ecc high unless given)} — a re-encoded code keeps its style unless given → {id, content, logo?}",
        has_doc,
        qr_code
    )]
}

/// The string a scanner reads, per InDesign's QR types.
fn encode(p: &Value) -> std::result::Result<String, String> {
    let g = |k: &str| str_param(p, k).unwrap_or("").trim().to_string();
    let esc = |s: String| s.replace('\\', r"\\").replace(';', r"\;").replace(':', r"\:").replace(',', r"\,");
    let out = match str_param(p, "type").unwrap_or("url") {
        "url" | "text" => g("content"),
        "sms" => format!("SMSTO:{}:{}", g("number"), g("message")),
        "email" => format!("MATMSG:TO:{};SUB:{};BODY:{};;", esc(g("to")), esc(g("subject")), esc(g("body"))),
        "vcard" => {
            let mut v = format!("MECARD:N:{};", esc(g("name")));
            for (k, tag) in [("org", "ORG"), ("phone", "TEL"), ("email", "EMAIL"), ("url", "URL")] {
                if !g(k).is_empty() {
                    v.push_str(&format!("{tag}:{};", esc(g(k))));
                }
            }
            v + ";"
        }
        t => return Err(format!("unknown type `{t}` (url, text, sms, email, vcard)")),
    };
    if out.is_empty() { Err("nothing to encode".into()) } else { Ok(out) }
}

/// The dialog fields a code's text was made from: [`encode`] the other way round (Edit QR Code
/// shows what the code holds).
pub fn decode(text: &str) -> Value {
    // Fields separated by unescaped `;`, unescaped.
    let split = |body: &str| -> Vec<String> {
        let mut out = vec![String::new()];
        let mut esc = false;
        for ch in body.chars() {
            match (esc, ch) {
                (true, c) => {
                    if let Some(last) = out.last_mut() {
                        last.push(c);
                    }
                    esc = false;
                }
                (false, '\\') => esc = true,
                (false, ';') => out.push(String::new()),
                (false, c) => {
                    if let Some(last) = out.last_mut() {
                        last.push(c);
                    }
                }
            }
        }
        out
    };
    let tagged = |body: &str, tags: &[(&str, &str)]| {
        let mut v = serde_json::Map::new();
        for field in split(body) {
            if let Some((tag, value)) = field.split_once(':')
                && let Some((_, key)) = tags.iter().find(|(t, _)| *t == tag)
            {
                v.insert((*key).into(), json!(value));
            }
        }
        v
    };
    if let Some(body) = text.strip_prefix("MECARD:") {
        let mut v = tagged(body, &[("N", "name"), ("ORG", "org"), ("TEL", "phone"), ("EMAIL", "email"), ("URL", "url")]);
        v.insert("type".into(), json!("vcard"));
        return Value::Object(v);
    }
    if let Some(body) = text.strip_prefix("MATMSG:") {
        let mut v = tagged(body, &[("TO", "to"), ("SUB", "subject"), ("BODY", "body")]);
        v.insert("type".into(), json!("email"));
        return Value::Object(v);
    }
    if let Some(body) = text.strip_prefix("SMSTO:") {
        let (number, message) = body.split_once(':').unwrap_or((body, ""));
        return json!({"type": "sms", "number": number, "message": message});
    }
    let kind = if text.contains("://") || text.starts_with("www.") { "url" } else { "text" };
    json!({"type": kind, "content": text})
}

/// What Edit QR Code starts from for the selected object, if it is (or holds) a generated code:
/// the dialog's fields for its content, style and colours.
pub fn edit_fields(d: &designcraft_doc::Document, sel: ItemId) -> Option<Value> {
    let (container, code) = code_place(d, sel)?;
    let c = d.item(code)?;
    let mut v = decode(c.alt_text.trim_start_matches(MARK));
    let q = c.qr.unwrap_or_default();
    let name = |x: Value| x.as_str().unwrap_or_default().to_string();
    v["ecc"] = json!(name(serde_json::to_value(q.ecc).unwrap_or_default()));
    v["modules"] = json!(name(serde_json::to_value(q.modules).unwrap_or_default()));
    v["finder"] = json!(name(serde_json::to_value(q.finder).unwrap_or_default()));
    v["roundness"] = json!(q.roundness);
    v["moduleSize"] = json!(q.scale);
    v["logoSize"] = json!(q.logo);
    v["color"] = json!(fill_swatch(c).unwrap_or_default());
    let siblings: Vec<std::sync::Arc<Item>> = container.and_then(|g| d.item(g)).map(|g| g.children().to_vec()).unwrap_or_default();
    let part = |k: QrPart| siblings.iter().find(|i| i.qr_part == Some(k)).and_then(|i| fill_swatch(i)).unwrap_or_default();
    v["finderColor"] = json!(part(QrPart::Finders));
    v["background"] = json!(part(QrPart::Background));
    v["editing"] = json!(true);
    Some(v)
}

/// The style `p` asks for, starting from `base` (a re-encoded code's own): every key given
/// replaces that part. A logo file without a size or error correction gets room and High.
fn style_param(p: &Value, base: QrStyle) -> std::result::Result<QrStyle, String> {
    let mut st = base;
    let word = |k: &str| str_param(p, k).map(str::to_ascii_lowercase);
    if let Some(e) = word("ecc") {
        st.ecc = match e.as_str() {
            "low" | "l" => Ecc::Low,
            "medium" | "m" => Ecc::Medium,
            "quartile" | "q" => Ecc::Quartile,
            "high" | "h" => Ecc::High,
            _ => return Err(format!("unknown ecc `{e}` (low, medium, quartile, high)")),
        };
    }
    if let Some(m) = word("modules") {
        st.modules = match m.as_str() {
            "square" => Modules::Square,
            "rounded" => Modules::Rounded,
            "dots" => Modules::Dots,
            "connected" => Modules::Connected,
            _ => return Err(format!("unknown modules `{m}` (square, rounded, dots, connected)")),
        };
    }
    if let Some(f) = word("finder") {
        st.finder = match f.as_str() {
            "square" => Finder::Square,
            "rounded" => Finder::Rounded,
            "circle" => Finder::Circle,
            _ => return Err(format!("unknown finder `{f}` (square, rounded, circle)")),
        };
    }
    let num = |k: &str| -> std::result::Result<Option<f64>, String> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).map(Some).ok_or_else(|| format!("`{k}` must be a number")),
        }
    };
    if let Some(v) = num("roundness")? {
        st.roundness = v;
    }
    if let Some(v) = num("moduleSize")? {
        st.scale = v;
    }
    match num("logoSize")? {
        Some(v) => st.logo = v,
        None if str_param(p, "logo").is_some() && st.logo <= 0.0 => st.logo = 0.25,
        None => {}
    }
    if str_param(p, "logo").is_some() && p.get("ecc").is_none() {
        st.ecc = Ecc::High;
    }
    Ok(st.sanitized())
}

fn qr_item(id: ItemId, layer: designcraft_doc::LayerId, text: &str, r: Rect, color: &str, style: &QrStyle) -> std::result::Result<Item, String> {
    let path = designcraft_geom::qr::qr_styled_path(text, r, style).ok_or("too much content for a QR code")?;
    let mut it = Item::new(id, layer, Shape::Path, path);
    it.qr = (!style.is_classic()).then_some(*style);
    it.fill = Fill::swatch(color);
    it.alt_text = format!("{MARK}{text}");
    Ok(it)
}

/// The colours of a code's parts: the code, its finder patterns (with the code when `None`) and
/// its background (none when `None`). Each is a swatch: a colour, a tint or a gradient.
#[derive(Clone, Debug, PartialEq)]
struct Colors {
    code: String,
    finders: Option<String>,
    background: Option<String>,
}

impl Colors {
    /// The colours `p` gives, over `base` (an edited code's own). `finderColor`/`background` of
    /// `""` or `none` take that part away.
    fn from(p: &Value, base: Colors) -> Colors {
        let part = |k: &str, cur: Option<String>| match str_param(p, k) {
            Some(v) if v.is_empty() || v.eq_ignore_ascii_case("none") || v == "[None]" => None,
            Some(v) => Some(v.to_string()),
            None => cur,
        };
        Colors {
            code: str_param(p, "color").map_or(base.code, str::to_string),
            finders: part("finderColor", base.finders),
            background: part("background", base.background),
        }
    }
}

/// The objects a code in `r` is made of, bottom to top: its background, the code (with its
/// finder patterns unless they have a colour of their own), the finder patterns. One object
/// when the code has one colour and no background.
fn qr_parts(
    d: &mut designcraft_doc::Document,
    layer: designcraft_doc::LayerId,
    text: &str,
    r: Rect,
    style: &QrStyle,
    colors: &Colors,
    code_id: Option<ItemId>,
) -> std::result::Result<Vec<Item>, String> {
    let shapes = designcraft_geom::qr::qr_shapes(text, r, style).ok_or("too much content for a QR code")?;
    let mut out = Vec::new();
    if let Some(bg) = &colors.background {
        let mut it = Item::new(ItemId(d.alloc()), layer, Shape::Rectangle, designcraft_geom::shapes::rectangle(shapes.frame));
        it.fill = Fill::swatch(bg);
        it.qr_part = Some(QrPart::Background);
        out.push(it);
    }
    let id = code_id.unwrap_or_else(|| ItemId(d.alloc()));
    match &colors.finders {
        None => out.push(qr_item(id, layer, text, r, &colors.code, style)?),
        Some(fc) => {
            let mut code = qr_item(id, layer, text, r, &colors.code, style)?;
            code.path = shapes.modules;
            out.push(code);
            let mut it = Item::new(ItemId(d.alloc()), layer, Shape::Path, shapes.finders);
            it.fill = Fill::swatch(fc);
            it.qr_part = Some(QrPart::Finders);
            out.push(it);
        }
    }
    Ok(out)
}

/// The swatch an object is filled with, if it is filled with one.
fn fill_swatch(it: &Item) -> Option<String> {
    (!it.fill.is_none()).then(|| it.fill.swatch.clone())
}

/// The group or frame an object sits in, if any (not the spread).
fn parent_of(d: &designcraft_doc::Document, id: ItemId) -> Option<ItemId> {
    let loc = d.find(id)?;
    let (_, up) = loc.path.split_last()?;
    let mut items = &d.spread(loc.spread)?.items[..];
    let mut parent = None;
    for &i in up {
        let it = items.get(i)?;
        parent = Some(it.id);
        items = it.children();
    }
    parent
}

/// Where a code sits: the objects of its group or frame (`Some(container)`), or of the spread.
/// Returns the container and the code's index among them.
fn code_place(d: &designcraft_doc::Document, sel: ItemId) -> Option<(Option<ItemId>, ItemId)> {
    let it = d.item(sel)?;
    if it.alt_text.starts_with(MARK) {
        return Some((parent_of(d, sel), sel));
    }
    // A group or frame holding a code: the code among its objects.
    let code = it.children().iter().find(|c| c.alt_text.starts_with(MARK))?;
    Some((Some(sel), code.id))
}

fn qr_code(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "object.qrCode";
    let text = encode(p).map_err(|e| bad(ID, e))?;
    let rect = rect_param(p, "rect");
    let sr = spread_param(p, ID)?;
    let logo_file = str_param(p, "logo").map(str::to_string);
    let st = s.doc()?;
    let lid = st.active_layer;
    let sel = st.selection.items.clone();
    let editing = if rect.is_some() { None } else { sel.first().and_then(|t| code_place(&st.doc, *t)) };
    // An edited code keeps its style and colours unless the params change them.
    let (base_style, base_colors) = match editing.and_then(|(container, code)| {
        let d = &st.doc;
        let c = d.item(code)?;
        let siblings: Vec<std::sync::Arc<Item>> = match container {
            Some(g) => d.item(g)?.children().to_vec(),
            None => Vec::new(),
        };
        let part = |k: QrPart| siblings.iter().find(|i| i.qr_part == Some(k)).and_then(|i| fill_swatch(i));
        Some((
            c.qr.unwrap_or_default(),
            Colors {
                code: fill_swatch(c).unwrap_or_else(|| designcraft_color::swatch::BLACK.into()),
                finders: part(QrPart::Finders),
                background: part(QrPart::Background),
            },
        ))
    }) {
        Some(b) => b,
        None => (QrStyle::default(), Colors { code: designcraft_color::swatch::BLACK.into(), finders: None, background: None }),
    };
    let style = style_param(p, base_style).map_err(|e| bad(ID, e))?;
    let colors = Colors::from(p, base_colors);
    // (the object to select, the code, where the logo goes in the code's space, objects to group)
    let (id, code, logo, group) = s.edit(|d, selection| {
        for c in [Some(&colors.code), colors.finders.as_ref(), colors.background.as_ref()].into_iter().flatten() {
            if d.swatch(c).is_none() {
                return Err(bad(ID, format!("no swatch `{c}`")));
            }
        }
        let logo_in = |r: Rect| designcraft_geom::qr::qr_shapes(&text, r, &style).and_then(|q| q.logo);
        let made = match (rect, sel.first().copied(), editing) {
            (Some(r), _, _) => {
                let parts = qr_parts(d, lid, &text, r, &style, &colors, None).map_err(|e| bad(ID, e))?;
                let code = parts.iter().find(|i| i.alt_text.starts_with(MARK)).map(|i| i.id).ok_or_else(|| bad(ID, "no code"))?;
                let ids: Vec<ItemId> = parts.iter().map(|i| i.id).collect();
                for it in parts {
                    d.insert_item(sr, it, None)?;
                }
                (code, code, logo_in(r), (ids.len() > 1).then_some(ids))
            }
            (None, _, Some((container, code_id))) => {
                // Edit QR Code: the same square, the parts drawn anew; a logo and other objects stay.
                let c = d.item(code_id).ok_or(designcraft_doc::DocError::NoItem(code_id))?.clone();
                let siblings: Vec<std::sync::Arc<Item>> = match container {
                    Some(g) => d.item(g).map(|g| g.children().to_vec()).unwrap_or_default(),
                    None => vec![std::sync::Arc::new(c.clone())],
                };
                let finders = siblings.iter().find(|i| i.qr_part == Some(QrPart::Finders));
                let symbol = finders.and_then(|f| f.path.bounds()).into_iter().chain(c.path.bounds()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
                let n = designcraft_geom::qr::qr_size_at(c.alt_text.trim_start_matches(MARK), c.qr.unwrap_or_default().ecc).unwrap_or(21);
                let quiet = symbol.width() / n as f64 * 2.0;
                let side = symbol.width().max(symbol.height()) + 2.0 * quiet;
                let r = Rect::from_center_size(symbol.center(), (side, side));
                let parts = qr_parts(d, c.layer, &text, r, &style, &colors, Some(code_id)).map_err(|e| bad(ID, e))?;
                let new_ids: Vec<ItemId> = parts.iter().map(|i| i.id).collect();
                let is_part = |i: &Item| i.id == code_id || i.qr_part.is_some();
                match container {
                    Some(g) => {
                        let items = d.item_mut(g).and_then(Item::children_mut).ok_or(designcraft_doc::DocError::NoItem(g))?;
                        let at = items.iter().position(|i| is_part(i)).unwrap_or(0);
                        items.retain(|i| !is_part(i));
                        for (k, it) in parts.into_iter().enumerate() {
                            items.insert((at + k).min(items.len()), std::sync::Arc::new(it));
                        }
                        (sel.first().copied().unwrap_or(g), code_id, logo_in(r), None)
                    }
                    None => {
                        // A lone code: replaced where it is; more parts than one become a group.
                        let loc = d.find(code_id).ok_or(designcraft_doc::DocError::NoItem(code_id))?;
                        let spread = loc.spread;
                        let at = loc.path.first().copied();
                        d.remove_item(code_id)?;
                        for (k, it) in parts.into_iter().enumerate() {
                            d.insert_item(spread, it, at.map(|i| i + k))?;
                        }
                        (code_id, code_id, logo_in(r), (new_ids.len() > 1).then_some(new_ids))
                    }
                }
            }
            (None, Some(t), None) => {
                // Into the frame: the code fills it, as content.
                let it = d.item(t).ok_or_else(|| bad(ID, "no such object"))?.clone();
                if !matches!(it.content, Content::Unassigned | Content::Group { .. }) || it.shape == Shape::Group {
                    return Err(bad(ID, "select an empty frame, a QR code, or give `rect`"));
                }
                let r = it.inner_bounds();
                let parts = qr_parts(d, it.layer, &text, r, &style, &colors, None).map_err(|e| bad(ID, e))?;
                let code = parts.iter().find(|i| i.alt_text.starts_with(MARK)).map(|i| i.id).ok_or_else(|| bad(ID, "no code"))?;
                let f = d.item_mut(t).ok_or(designcraft_doc::DocError::NoItem(t))?;
                f.content = Content::Group { items: parts.into_iter().map(std::sync::Arc::new).collect() };
                (t, code, logo_in(r), None)
            }
            (None, None, _) => return Err(bad(ID, "give `rect` or select a frame")),
        };
        *selection = Selection::items(vec![made.0]);
        Ok(made)
    })?;
    let mut out = json!({"id": id.0, "content": text});
    let mut id = id;
    // A code in more than one part: one group (part of this undo step, as all this command runs).
    if let Some(ids) = group {
        let g = s.execute("object.group", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))?;
        if let Some(g) = g["id"].as_u64() {
            id = ItemId(g);
            out["id"] = json!(g);
        }
    }
    // The logo: a frame on the clear area with the image fitted in it, grouped with the code.
    if let (Some(file), Some(area)) = (logo_file, logo) {
        let d = &s.doc()?.doc;
        let loc = d.find(code).ok_or(designcraft_doc::DocError::NoItem(code))?;
        let to_spread = d.parent_xf(&loc) * d.item(code).ok_or(designcraft_doc::DocError::NoItem(code))?.xf;
        let a = to_spread.transform_rect_bbox(area);
        let spread = match loc.spread {
            designcraft_doc::SpreadRef::Doc(i) => json!({"kind": "doc", "index": i}),
            designcraft_doc::SpreadRef::Parent(i) => json!({"kind": "parent", "index": i}),
        };
        let frame = s.execute("frame.create", &json!({"rect": [a.x0, a.y0, a.x1, a.y1], "content": "unassigned", "spread": spread}))?["id"].clone();
        s.execute("file.place", &json!({"path": file, "frame": frame}))?;
        s.execute("object.fit", &json!({"mode": "fitProportionally", "ids": [frame]}))?;
        let group = s.execute("object.group", &json!({"ids": [id.0, frame]}))?;
        out["logo"] = frame;
        out["id"] = group["id"].clone();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use designcraft_doc::Content;
    use designcraft_geom::qr::{Ecc, Finder, Modules};
    use serde_json::json;

    use super::MARK;

    use crate::Session;

    #[test]
    fn generate_and_edit_qr_codes() {
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let r = s.execute("object.qrCode", &json!({"content": "https://example.com", "rect": [72, 72, 216, 216]})).unwrap();
        let id = designcraft_doc::ItemId(r["id"].as_u64().unwrap());
        let item = |s: &Session| s.doc().unwrap().doc.item(id).unwrap().clone();
        let a = item(&s);
        assert!(a.path.subpaths.len() > 20 && a.alt_text.contains("example.com"));
        let b0 = a.bounds();
        assert!(b0.x0 >= 72.0 && b0.x1 <= 216.0);
        // Edit QR Code: new content, same place.
        let r = s.execute("object.qrCode", &json!({"type": "email", "to": "a@b.c", "subject": "Hi"})).unwrap();
        assert_eq!(r["content"], "MATMSG:TO:a@b.c;SUB:Hi;BODY:;;");
        let b1 = item(&s).bounds();
        assert!((b1.center() - b0.center()).hypot() < 1.0, "{b0:?} → {b1:?}");
        assert!((b1.width() - b0.width()).abs() < 4.0);
        // Into an empty frame.
        let f = s.execute("frame.create", &json!({"rect": [300, 300, 400, 400], "content": "unassigned"})).unwrap()["id"].as_u64().unwrap();
        s.execute("object.qrCode", &json!({"type": "vcard", "name": "Ada", "phone": "+44 1"})).unwrap();
        let fr = s.doc().unwrap().doc.item(designcraft_doc::ItemId(f)).unwrap().clone();
        assert!(fr.has_nested_items());
        assert!(s.execute("object.qrCode", &json!({"type": "fax", "content": "x", "rect": [0, 0, 9, 9]})).is_err());
    }

    /// A styled code keeps its style (stored on the object) when its content is edited, and a
    /// classic code stores none.
    #[test]
    fn a_styled_code_keeps_its_style_when_edited() {
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let classic = s.execute("object.qrCode", &json!({"content": "a", "rect": [10, 10, 110, 110]})).unwrap()["id"].as_u64().unwrap();
        assert!(s.doc().unwrap().doc.item(designcraft_doc::ItemId(classic)).unwrap().qr.is_none());
        let r = s
            .execute(
                "object.qrCode",
                &json!({"content": "https://example.com", "rect": [200, 200, 300, 300], "modules": "dots", "finder": "circle", "ecc": "q", "moduleSize": 0.8, "roundness": 0.5}),
            )
            .unwrap();
        let id = designcraft_doc::ItemId(r["id"].as_u64().unwrap());
        let style = |s: &Session| s.doc().unwrap().doc.item(id).unwrap().qr.unwrap();
        let st = style(&s);
        assert_eq!((st.modules, st.finder, st.ecc, st.scale, st.roundness), (Modules::Dots, Finder::Circle, Ecc::Quartile, 0.8, 0.5));
        // Edit QR Code with new content only: the look stays; a given key still changes it.
        s.execute("selection.set", &json!({"ids": [id.0]})).unwrap();
        s.execute("object.qrCode", &json!({"content": "https://example.org"})).unwrap();
        assert_eq!(style(&s), st);
        s.execute("object.qrCode", &json!({"content": "https://example.org", "modules": "connected"})).unwrap();
        assert_eq!(style(&s).modules, Modules::Connected);
        for bad in [
            json!({"content": "x", "rect": [0, 0, 9, 9], "modules": "hearts"}),
            json!({"content": "x", "rect": [0, 0, 9, 9], "ecc": "max"}),
            json!({"content": "x", "rect": [0, 0, 9, 9], "roundness": "round"}),
        ] {
            assert!(s.execute("object.qrCode", &bad).is_err(), "{bad}");
        }
    }

    /// A logo file goes on the clear area in the middle, fitted, grouped with the code, at High
    /// error correction, and the whole is one undo step.
    #[test]
    fn a_logo_is_placed_in_the_middle_in_one_undo_step() {
        let dir = std::env::temp_dir().join(format!("dc-qr-logo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("logo.png");
        std::fs::write(&png, designcraft_render::Rendered { width: 40, height: 40, pixels: vec![200u8; 40 * 40 * 4] }.to_png()).unwrap();
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let undo = |s: &Session| s.doc().unwrap().history.undo.len();
        let before = undo(&s);
        let r = s
            .execute("object.qrCode", &json!({"content": "https://example.com", "rect": [100, 100, 300, 300], "logo": png.to_string_lossy()}))
            .unwrap();
        assert_eq!(undo(&s), before + 1, "one undo step");
        let d = &s.doc().unwrap().doc;
        let group = d.item(designcraft_doc::ItemId(r["id"].as_u64().unwrap())).unwrap();
        assert!(group.is_group());
        let logo = d.item(designcraft_doc::ItemId(r["logo"].as_u64().unwrap())).unwrap();
        assert!(matches!(logo.content, Content::Graphic(_)), "the image is in the logo frame");
        let code = group.children().iter().find(|c| c.alt_text.starts_with(MARK)).unwrap();
        assert_eq!(code.qr.unwrap().ecc, Ecc::High, "High unless given");
        assert!((code.qr.unwrap().logo - 0.25).abs() < 1e-9);
        let b = logo.bounds();
        assert!((b.center() - designcraft_geom::Point::new(200.0, 200.0)).hypot() < 1.0, "centred: {b:?}");
        assert!(b.width() < 200.0 * 0.3, "within the clear area: {b:?}");
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.doc().unwrap().doc.spreads[0].items.is_empty(), "undo takes code and logo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A code with a background and finder patterns in a colour of their own is a group of three
    /// ordinary objects; a gradient swatch colours the code; editing keeps the colours; a one-colour
    /// code given a background on editing becomes a group in its place.
    #[test]
    fn coloured_parts_are_ordinary_objects_that_editing_keeps() {
        use designcraft_geom::qr::QrPart;
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        // A gradient swatch to colour the code with.
        let g = s.execute("frame.create", &json!({"rect": [500, 500, 520, 520], "content": "unassigned"})).unwrap()["id"].as_u64().unwrap();
        s.execute("object.gradient", &json!({"ids": [g], "stops": [{"location": 0, "color": {"c": 100, "m": 0, "y": 0, "k": 0}}, {"location": 100, "color": {"c": 0, "m": 100, "y": 0, "k": 0}}]})).unwrap();
        s.execute("selection.set", &json!({"ids": [g]})).unwrap();
        let grad = s.execute("swatch.new", &json!({})).unwrap()["name"].as_str().unwrap().to_string();
        let r = s
            .execute(
                "object.qrCode",
                &json!({"content": "https://example.com", "rect": [100, 100, 200, 200], "color": grad, "finderColor": "C=100 M=0 Y=0 K=0", "background": "[Paper]"}),
            )
            .unwrap();
        let gid = designcraft_doc::ItemId(r["id"].as_u64().unwrap());
        let parts = |s: &Session, id: designcraft_doc::ItemId| -> Vec<(Option<QrPart>, bool, String)> {
            s.doc()
                .unwrap()
                .doc
                .item(id)
                .unwrap()
                .children()
                .iter()
                .map(|c| (c.qr_part, c.alt_text.starts_with(MARK), c.fill.swatch.clone()))
                .collect()
        };
        assert_eq!(
            parts(&s, gid),
            [
                (Some(QrPart::Background), false, "[Paper]".to_string()),
                (None, true, grad.clone()),
                (Some(QrPart::Finders), false, "C=100 M=0 Y=0 K=0".to_string())
            ],
            "background, code, finder patterns"
        );
        // Edit with new content only: the same parts and colours.
        s.execute("selection.set", &json!({"ids": [gid.0]})).unwrap();
        s.execute("object.qrCode", &json!({"content": "https://example.org/a/longer/address"})).unwrap();
        assert_eq!(parts(&s, gid).iter().map(|p| (p.0, p.1, p.2.clone())).collect::<Vec<_>>()[1].2, grad);
        assert_eq!(parts(&s, gid).len(), 3);
        // The finder patterns back to the code's colour: that part goes.
        s.execute("object.qrCode", &json!({"content": "https://example.org", "finderColor": "none"})).unwrap();
        assert_eq!(parts(&s, gid).iter().filter(|p| p.0 == Some(QrPart::Finders)).count(), 0);
        // A lone one-colour code given a background becomes a group where it was.
        let lone = s.execute("object.qrCode", &json!({"content": "x", "rect": [300, 100, 400, 200]})).unwrap()["id"].as_u64().unwrap();
        s.execute("selection.set", &json!({"ids": [lone]})).unwrap();
        let r = s.execute("object.qrCode", &json!({"content": "x", "background": "[Paper]"})).unwrap();
        let d = &s.doc().unwrap().doc;
        let group = d.item(designcraft_doc::ItemId(r["id"].as_u64().unwrap())).unwrap();
        assert!(group.is_group() && group.children().len() == 2, "{:?}", group.children().len());
        assert!(s.execute("object.qrCode", &json!({"content": "x", "rect": [0, 0, 9, 9], "background": "Nope"})).is_err());
    }

    /// Edit QR Code shows what a code holds: every type's fields come back from its text.
    #[test]
    fn decode_gives_back_the_fields_encode_was_given() {
        for p in [
            json!({"type": "url", "content": "https://example.com/a?b=c"}),
            json!({"type": "text", "content": "Hello; world"}),
            json!({"type": "sms", "number": "+49 170 1", "message": "Hi there"}),
            json!({"type": "email", "to": "a@b.c", "subject": "Re: one; two", "body": "x"}),
            json!({"type": "vcard", "name": "Ada Lovelace", "org": "Analytical; Engines", "phone": "+44 1", "email": "ada@x.y", "url": "https://x.y"}),
        ] {
            let text = super::encode(&p).unwrap();
            let back = super::decode(&text);
            for (k, v) in p.as_object().unwrap() {
                assert_eq!(&back[k], v, "{k} of {p}");
            }
        }
    }
}
