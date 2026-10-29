use agentfx_script::{parse::parse, print::print, json::{to_json, from_json}, ast::*};

const EXAMPLE: &str = r#"video "Neu im Studio" size=1920x1080 fps=30 voice=en_US-ljspeech-medium theme=studio-dark
// a comment
scene panels title="Panels"
  browser #app url=studio.example.test shot=shots/panels.png hotspots=shots/panels.json
  say "Das rechte Panel hat jetzt [[Reiter]]. Zahlen zeigen, wo etwas [[offen]] ist."
    @Reiter zoom #tabs z=1.9 ; cursor -> #tabs
    @offen  pulse #badge-open
  anim #logo scale 0s:0.85 0.5s:1~spring(170, 26) opacity 0s:0 0.3s:1
  code #snip lang=ts |
    const a = 1;   // body lines are raw, comments kept
    -const b = 2;
    +const b = 3;
  chart #c type=bar data=[3, 5, 8] labels=["Jan", "Feb", "Mär"]
"#;

fn str_value(s: &str) -> Value { Value::Str(s.into()) }

fn clear_spans(mut doc: Document) -> Document {
    fn clear(nodes: &mut [Node]) {
        for node in nodes { node.span = Span::default(); clear(&mut node.children); }
    }
    clear(&mut doc.nodes);
    doc
}

fn roundtrip(src: &str) -> Document {
    let (doc, errors) = parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let text = print(&doc);
    assert!(text.ends_with('\n'));
    let (again, errors) = parse(&text);
    assert!(errors.is_empty(), "{text}\n{errors:?}");
    assert_eq!(clear_spans(doc.clone()), clear_spans(again.clone()), "{text}");
    assert_eq!(print(&again), text);
    let json = to_json(&doc);
    let decoded = from_json(&json).unwrap();
    assert_eq!(clear_spans(doc.clone()), decoded);
    assert_eq!(print(&decoded), text);
    doc
}

#[test]
fn example_structure_and_roundtrips() {
    let doc = roundtrip(EXAMPLE);
    assert_eq!(doc.reel, "0");
    assert_eq!(doc.nodes.len(), 2);
    assert_eq!(doc.nodes[0].props["fps"], Value::Num(30.0));
    let children = &doc.nodes[1].children;
    assert_eq!(children.len(), 5);
    let at = &children[1].children[0];
    assert_eq!(at.kind, "at");
    assert_eq!(at.args, vec![str_value("Reiter")]);
    assert_eq!(at.span, Span { line: 6, col: 5 });
    assert_eq!(at.children.len(), 2);
    assert_eq!(at.children[0].kind, "zoom");
    assert_eq!(at.children[0].props["z"], Value::Num(1.9));
    assert_eq!(at.children[1].kind, "cursor");
    assert_eq!(at.children[1].args, vec![str_value("->"), str_value("#tabs")]);
    assert_eq!(children[2].args, ["#logo", "scale", "0s:0.85", "0.5s:1~spring(170, 26)", "opacity", "0s:0", "0.3s:1"].map(str_value).to_vec());
    assert_eq!(children[3].body.as_deref(), Some("const a = 1;   // body lines are raw, comments kept\n-const b = 2;\n+const b = 3;"));
    assert!(children[3].children.is_empty());
    assert_eq!(children[4].props["data"], Value::List(vec![Value::Num(3.0), Value::Num(5.0), Value::Num(8.0)]));
    assert_eq!(children[4].props["labels"], Value::List(vec![str_value("Jan"), str_value("Feb"), str_value("Mär")]));
    assert!(print(&doc).contains("@Reiter zoom #tabs z=1.9 ; cursor -> #tabs"));
}

#[test]
fn comments_urls_escapes_and_lists() {
    let doc = roundtrip(r#"// ignored
browser url=https://x.y path=a//b text="// still here" // ignored
text "12" "" "a\"b\\c\n\t" background=linear(90deg, #fff 0%, #000 100%)
chart data=[1 2, ["3", -0.5 .5], "a,b", //literal] expr=call("a ; b", //literal)
"#);
    assert_eq!(doc.nodes.len(), 3);
    assert_eq!(doc.nodes[0].props["url"], str_value("https://x.y"));
    assert_eq!(doc.nodes[1].args[0], str_value("12"));
    assert_eq!(doc.nodes[1].args[2], str_value("a\"b\\c\n\t"));
    assert_eq!(doc.nodes[2].props["expr"], str_value("call(\"a ; b\", //literal)"));
}

#[test]
fn statements_anchors_indentation_and_spans() {
    let doc = roundtrip("scene a ; scene b\r\n\tgroup\r\n\t  text \"ü\" ; text second\r\n\t    pulse #x\r\n\t@\"Doppel klick\" zoom #x ; cursor -> #x\r\n\t  pulse #x\r\n");
    assert!(doc.nodes[0].children.is_empty());
    let children = &doc.nodes[1].children;
    assert_eq!(children[0].children[1].children[0].kind, "pulse");
    assert_eq!(children[1].args, vec![str_value("Doppel klick")]);
    assert_eq!(children[1].children.len(), 3);
    assert_eq!(doc.nodes[1].span, Span { line: 1, col: 11 });
    assert_eq!(children[0].span, Span { line: 2, col: 2 });
    roundtrip("@Reiter\n  group\n    pulse #x\n@2.5s a ; b ; c ; d\n@outer @inner a ; b\n");
}

#[test]
fn indentation_error_recovers_at_shallower_level() {
    let (doc, errors) = parse("scene a\n    group\n      text deep\n  text recovered\ntext end\n");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "E_INDENT");
    assert_eq!(errors[0].span, Span { line: 4, col: 3 });
    assert_eq!(errors[0].hint, "indent with 2 spaces per level; this line's indent matches no parent");
    assert_eq!(doc.nodes.len(), 3);
    assert_eq!(doc.nodes[1].args, vec![str_value("recovered")]);
}

#[test]
fn diagnostics_and_line_recovery() {
    let (doc, errors) = parse("text \"unclosed\n#x nope\n=value\nchart data=[1, 2\ntext x=\ntext x=1 x=2\ntext good\n");
    assert_eq!(errors.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(), ["E_STRING", "E_KIND", "E_KIND", "E_BRACKET", "E_EMPTY_VALUE", "E_DUP_PROP"]);
    assert_eq!(errors[0].hint, "close the string with \"");
    assert_eq!(doc.nodes.len(), 2);
    assert_eq!(doc.nodes[0].props["x"], Value::Num(2.0));
    assert_eq!(doc.nodes[1].args, vec![str_value("good")]);
}

#[test]
fn raw_bodies_keep_comments_and_blank_lines() {
    let doc = roundtrip("code #a ; code #b |\n\n\t  // raw\n\n\t    +added; x=y\n\n\ntext after\ncode empty |\n");
    assert_eq!(doc.nodes[1].body.as_deref(), Some("\n// raw\n\n  +added; x=y"));
    assert!(doc.nodes[1].children.is_empty());
    assert_eq!(doc.nodes[3].body.as_deref(), Some(""));
}

#[test]
fn canonical_quoting_and_precision() {
    let doc = roundtrip(r#"text "a=b" "|" "@Reiter" "[x]" "//x" "a;b" "a,b" "12" "a)b" "a b" x="12"
text 0.123456789 -0.000000001 1000000000000000
at 12
"#);
    assert!(print(&doc).starts_with("text \"a=b\" \"|\" \"@Reiter\""));
    assert_eq!(print(&roundtrip("text #a a=1 b=2\n")), "text #a a=1 b=2\n");
    assert_eq!(print(&roundtrip("")), "\n");
}

#[test]
fn json_normalization_shape_and_diagnostics() {
    let doc = from_json(r#"{"nodes":[{"kind":"text","args":[true,false,12,[true,"12"]],"props":{"z":false,"a":3}}]}"#).unwrap();
    assert_eq!(doc.reel, "0");
    assert_eq!(doc.nodes[0].args, vec![str_value("true"), str_value("false"), Value::Num(12.0), Value::List(vec![str_value("true"), str_value("12")])]);
    assert_eq!(doc.nodes[0].props.keys().map(String::as_str).collect::<Vec<_>>(), ["z", "a"]);
    assert_eq!(clear_spans(roundtrip(&print(&doc))), doc);
    assert!(!to_json(&doc).contains("span"));
    for src in ["null", r#"{"nodes":[{"kind":"text","args":[null]}]}"#] {
        assert_eq!(from_json(src).unwrap_err().code, "E_JSON_NULL");
    }
    for src in ["[]", r#"{"nodes":{}}"#, r#"{"nodes":[{"args":[]}]}"#] {
        let error = from_json(src).unwrap_err();
        assert_eq!(error.code, "E_JSON");
        assert_eq!(error.hint, "see schema/reel-script.schema.json");
    }
    let error = from_json("{\n  \"nodes\": [}").unwrap_err();
    assert_eq!(error.code, "E_JSON");
    assert_eq!(error.span.line, 2);
    assert!(error.span.col > 0);
}
