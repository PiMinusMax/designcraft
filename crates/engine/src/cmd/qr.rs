//! Object › Generate QR Code: a vector QR code (a compound path) as a new object, inside the
//! selected frame, or replacing a selected code (Edit QR Code).

use designcraft_doc::{Content, Fill, Item, ItemId, Selection, Shape};
use designcraft_geom::Rect;
use designcraft_geom::qr::{Ecc, Finder, Modules, QrStyle};
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
        "{type?: url|text|sms|email|vcard (default url), content (url/text), number?, message?, to?, subject?, body?, name?, phone?, email?, org?, url?, color? (swatch, [Black]), rect? (new object; else into the selected frame, or re-encode a selected code), spread?, ecc?: low|medium|quartile|high (error correction; high with a logo), modules?: square|rounded|dots|connected, roundness? (0–1), moduleSize? (0.5–1 of a cell: below 1 the modules stand apart), finder?: square|rounded|circle (the three corner patterns), logoSize? (0–0.34 of the side, left clear in the middle), logo? (an image file placed there, fitted; logoSize 0.25 and ecc high unless given)} — a re-encoded code keeps its style unless given → {id, content, logo?}",
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

fn qr_code(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "object.qrCode";
    let text = encode(p).map_err(|e| bad(ID, e))?;
    let color = str_param(p, "color").unwrap_or(designcraft_color::swatch::BLACK).to_string();
    let rect = rect_param(p, "rect");
    let sr = spread_param(p, ID)?;
    let logo_file = str_param(p, "logo").map(str::to_string);
    let st = s.doc()?;
    let lid = st.active_layer;
    let sel = st.selection.items.clone();
    // A selected code keeps its own style unless the params change it.
    let own = sel.first().and_then(|t| st.doc.item(*t)).filter(|it| it.alt_text.starts_with(MARK)).and_then(|it| it.qr);
    let style = style_param(p, if rect.is_some() { QrStyle::default() } else { own.unwrap_or_default() }).map_err(|e| bad(ID, e))?;
    // (the object to select, the code item, where the logo goes in the code's own space)
    let (id, code, logo) = s.edit(|d, selection| {
        if d.swatch(&color).is_none() {
            return Err(bad(ID, format!("no swatch `{color}`")));
        }
        let logo_in = |r: Rect| designcraft_geom::qr::qr_shapes(&text, r, &style).and_then(|q| q.logo);
        let target = if rect.is_some() { None } else { sel.first().copied() };
        let made = match (rect, target) {
            (Some(r), _) => {
                let id = ItemId(d.alloc());
                d.insert_item(sr, qr_item(id, lid, &text, r, &color, &style).map_err(|e| bad(ID, e))?, None)?;
                (id, id, logo_in(r))
            }
            (None, Some(t)) => {
                let it = d.item(t).ok_or_else(|| bad(ID, "no such object"))?.clone();
                if it.alt_text.starts_with(MARK) {
                    // Edit QR Code: same place and size.
                    let b = it.path.bounds().unwrap_or(Rect::ZERO);
                    let side = b.width().max(b.height());
                    let n = designcraft_geom::qr::qr_size_at(it.alt_text.trim_start_matches(MARK), it.qr.unwrap_or_default().ecc).unwrap_or(21);
                    let quiet = side / n as f64 * 2.0;
                    let r = Rect::from_center_size(b.center(), (side + 2.0 * quiet, side + 2.0 * quiet));
                    let fresh = qr_item(t, it.layer, &text, r, &color, &style).map_err(|e| bad(ID, e))?;
                    let it = d.item_mut(t).ok_or(designcraft_doc::DocError::NoItem(t))?;
                    it.path = fresh.path;
                    it.fill = fresh.fill;
                    it.alt_text = fresh.alt_text;
                    it.qr = fresh.qr;
                    (t, t, logo_in(r))
                } else {
                    // Into the frame: the code fills it, as content.
                    if !matches!(it.content, Content::Unassigned | Content::Group { .. }) || it.shape == Shape::Group {
                        return Err(bad(ID, "select an empty frame, a QR code, or give `rect`"));
                    }
                    let id = ItemId(d.alloc());
                    let r = it.inner_bounds();
                    let code = qr_item(id, it.layer, &text, r, &color, &style).map_err(|e| bad(ID, e))?;
                    let f = d.item_mut(t).ok_or(designcraft_doc::DocError::NoItem(t))?;
                    f.content = Content::Group { items: vec![std::sync::Arc::new(code)] };
                    (t, id, logo_in(r))
                }
            }
            (None, None) => return Err(bad(ID, "give `rect` or select a frame")),
        };
        *selection = Selection::items(vec![made.0]);
        Ok(made)
    })?;
    let mut out = json!({"id": id.0, "content": text});
    // The logo: a frame on the clear area with the image fitted in it, grouped with the code (all
    // one undo step: what this command runs is part of it).
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
}
