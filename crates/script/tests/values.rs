use agentfx_script::values::*;

fn near(a: f64, b: f64) { assert!((a-b).abs() < 1e-8, "{a} != {b}"); }
fn lit(s: &str) -> Rgba {
    match parse_color(s).unwrap() { ColorRef::Lit(c) => c, other => panic!("expected literal: {other:?}") }
}
fn code<T: std::fmt::Debug>(result: Result<T, ValueError>, expected: &str) {
    let e = result.unwrap_err(); assert_eq!(e.code, expected); assert!(!e.hint.is_empty());
}
#[test]
fn durations() {
    for (s, expected) in [("1.5s",1.5), ("300ms",0.3), (" 0.4 S ",0.4), ("1min",60.0), ("-0.2s",-0.2), ("2",2.0)] {
        near(parse_dur(s).unwrap().secs(24.0),expected);
    }
    assert_eq!(parse_dur("16f").unwrap(), Dur::Frames(16.0));
    near(Dur::Frames(12.0).secs(24.0),0.5);
    for s in ["", "1hour", "NaN", "inf", "1e309s", "1e308min"] { code(parse_dur(s),"E_DUR"); }
}
#[test]
fn anchors() {
    let a = parse_anchor(" @Veröffentlichen.end - 300ms ").unwrap();
    assert_eq!(a.base,AnchorBase::Name("Veröffentlichen".into()));
    assert!(a.end_edge); assert_eq!(a.offset,Some(Dur::Secs(-0.3)));
    assert_eq!(parse_anchor("say1").unwrap().base,AnchorBase::Name("say1".into()));
    assert_eq!(parse_anchor("a-long-name").unwrap().base,AnchorBase::Name("a-long-name".into()));
    for (s,b) in [("START",AnchorBase::SceneStart),("end",AnchorBase::SceneEnd),("prev",AnchorBase::Prev)] {
        assert_eq!(parse_anchor(s).unwrap().base,b);
    }
    assert_eq!(parse_anchor("12f+6f").unwrap().offset,Some(Dur::Frames(6.0)));
    assert_eq!(parse_anchor("1e-3s").unwrap().base,AnchorBase::Time(Dur::Secs(0.001)));
    for s in ["", "@", "@@name", "2name", "word.middle", "word+bad"] { code(parse_anchor(s),"E_ANCHOR"); }
}
#[test]
fn colors_and_contrast() {
    for s in ["#fff", "#ffffff", "WHITE", "rgb(255,255,255)", "rgb(100% 100% 100%)", "hsl(0 0% 100%)"] {
        assert_eq!(lit(s).to_css(),"#ffffff");
    }
    near(lit("#1234").a,68.0/255.0);
    near(lit("#11223344").a,68.0/255.0);
    near(lit("rgba(255, 0, 0, 0.5)").a,0.5);
    assert_eq!(lit("rgb(255 0 0 / 50%)").to_css(),"rgb(255 0 0 / 0.5)");
    assert_eq!(lit("hsl(120 100% 50% / 50%)").to_css(),"rgb(0 255 0 / 0.5)");
    assert_eq!(lit("hsl(0.5turn,100%,50%)").to_css(),"#00ffff");
    near(lit("transparent").a,0.0);
    assert_eq!(parse_color("$accent").unwrap(),ColorRef::Token("accent".into()));
    near(contrast_ratio(&lit("white"),&lit("black")),21.0);
    near(contrast_ratio(&lit("black"),&lit("white")),21.0);
    for s in ["#a", "#ggg", "#💚", "rgb(300 0 0)", "rgb(1,,2,3)", "rgb(1 2 3 / 2)", "hsl(0 1 1)", "$", "unknown"] {
        code(parse_color(s),"E_COLOR");
    }
}
#[test]
fn paints() {
    assert!(matches!(parse_paint("$accent").unwrap(),Paint::Solid(ColorRef::Token(_))));
    match parse_paint("linear(155deg, rgb(12, 42, 31) 0%, #17845a 140%)").unwrap() {
        Paint::Linear { angle_deg, stops } => { near(angle_deg,155.0); assert_eq!(stops.len(),2); assert_eq!(stops[1].pos,Some(1.4)); }
        p => panic!("unexpected {p:?}"),
    }
    assert!(matches!(parse_paint("linear(red, blue)").unwrap(),Paint::Linear { angle_deg: 180.0, .. }));
    assert!(matches!(parse_paint("radial(#fff, #000)").unwrap(),Paint::Radial { cx: 0.5, cy: 0.5, .. }));
    assert!(matches!(parse_paint("radial(at 30% 40%, white, black)").unwrap(),Paint::Radial { cx: 0.3, cy: 0.4, .. }));
    assert!(matches!(parse_paint("conic(0deg, red, blue)").unwrap(),Paint::Conic { angle_deg: 0.0, .. }));
    assert!(matches!(parse_paint("mesh(2x2, red, blue, white, black)").unwrap(),Paint::Mesh { cols: 2, rows: 2, .. }));
    assert_eq!(parse_paint("image(path.png)").unwrap(),Paint::Image { src: "path.png".into(), fit: "cover".into() });
    assert_eq!(parse_paint("image('my path.png' contain)").unwrap(),Paint::Image { src: "my path.png".into(), fit: "contain".into() });
    for s in ["linear(red)", "linear(red,)", "linear(rgb(1,2,3), blue", "mesh(2x2, red)", "radial(at 130% 40%, red, blue)", "image()"] {
        code(parse_paint(s),"E_PAINT");
    }
    assert!(parse_paint("liner(red,blue)").unwrap_err().hint.contains("did you mean"));
}
#[test]
fn easing_parsing_and_sampling() {
    let e = parse_ease("inout-cubic").unwrap();
    near(e.sample(0.0),0.0); near(e.sample(0.5),0.5); near(e.sample(1.0),1.0);
    assert_eq!(e,parse_ease("IN_OUT_CUBIC").unwrap());
    assert_eq!(e,parse_ease("in-out-cubic").unwrap());
    assert_eq!(e,parse_ease("inoutcubic").unwrap());
    near(parse_ease("cubic(0, 0, 1, 1)").unwrap().sample(0.3),0.3);
    near(parse_ease("out-cubic").unwrap().sample(0.5),0.8722017480412667);
    near(Ease::Hold.sample(0.99),0.0); near(Ease::Hold.sample(1.0),1.0);
    near(Ease::Steps(4).sample(0.49),0.25);
    assert_eq!(parse_ease("step").unwrap(),Ease::Steps(1));
    assert_eq!(parse_ease("bounce").unwrap(),Ease::Bounce { bounces: 3 });
    for family in ["sine","quad","cubic","quart","expo","back"] {
        for direction in ["in","out","inout"] { assert!(parse_ease(&format!("{direction}-{family}")).is_ok()); }
    }
    for s in ["steps(0)","steps(1.5)","spring(0,26)","spring(170,-1)","spring(170,26,0)","cubic(2,0,1,1)","bounce(0)","spring(1,2,3,4,5)"] {
        code(parse_ease(s),"E_EASE");
    }
    let e = parse_ease("bounse").unwrap_err(); assert_eq!(e.code,"E_EASE"); assert!(e.hint.contains("bounce"));
}
#[test]
fn spring_physics_and_css() {
    let e = parse_ease("spring(170,26)").unwrap();
    assert!(e.is_physical());
    let duration = e.natural_duration().unwrap(); assert!((0.5..1.2).contains(&duration),"{duration}");
    let (duration2, points) = spring_curve(170.0,26.0,1.0,0.0);
    near(duration,duration2); assert!(points.len() >= 60);
    assert_eq!(points[0],(0.0,0.0)); near(points.last().unwrap().0,1.0);
    assert!((points.last().unwrap().1-1.0).abs() < 0.001);
    let spacing = points[1].0-points[0].0;
    for p in points.windows(2) { near(p[1].0-p[0].0,spacing); }
    let bouncy = parse_ease("bouncy").unwrap();
    assert!((1..100).any(|i| bouncy.sample(f64::from(i)/100.0) > 1.0));
    assert_ne!(spring_curve(170.0,26.0,1.0,0.0),spring_curve(170.0,26.0,1.0,5.0));
    for spring in ["snappy", "soft", "spring(170,26,2,-1)"] { assert!(parse_ease(spring).unwrap().natural_duration().is_some()); }
    let css = ease_to_css(&e); assert!(css.starts_with("linear(0")); assert!(css.ends_with("1)"));
    assert!((40..=64).contains(&css.split(',').count()));
    assert_eq!(ease_to_css(&Ease::Hold),"step-end");
    assert_eq!(ease_to_css(&Ease::Steps(3)),"steps(3, end)");
    assert_eq!(ease_to_css(&Ease::Linear),"linear");
    assert_eq!(ease_to_css(&Ease::Cubic(0.0,0.0,1.0,1.0)),"cubic-bezier(0, 0, 1, 1)");
    let bounce = parse_ease("bounce(4)").unwrap();
    assert!(!bounce.is_physical()); assert_eq!(bounce.natural_duration(),None);
    near(bounce.sample(0.0),0.0); near(bounce.sample(1.0),1.0);
    assert!(ease_to_css(&bounce).ends_with("1)"));
}
#[test]
fn dimensions_and_numbers() {
    for (s,unit,v) in [("120",Unit::Px,120.0),("120 px",Unit::Px,120.0),("50%",Unit::Pct,50.0),("10vw",Unit::Vw,10.0),("2em",Unit::Em,2.0),("3VH",Unit::Vh,3.0)] {
        assert_eq!(parse_length(s).unwrap(),Length { v, unit });
    }
    near(parse_angle_deg("0.25turn").unwrap(),90.0);
    near(parse_angle_deg("1rad").unwrap(),180.0/std::f64::consts::PI);
    near(parse_angle_deg("30").unwrap(),30.0);
    near(parse_number("1.9x").unwrap(),1.9); near(parse_number("50%").unwrap(),0.5);
    assert_eq!(parse_size("1920 x 1080").unwrap(),(1920,1080));
    assert_eq!(parse_aspect("9:16").unwrap(),(9,16)); assert_eq!(parse_aspect("16x9").unwrap(),(16,9));
    near(parse_rate_pct("+6%").unwrap(),6.0); near(parse_rate_pct("-10%").unwrap(),-10.0);
    code(parse_length("1pt"),"E_LENGTH"); code(parse_angle_deg("1degree"),"E_ANGLE");
    code(parse_number("NaN"),"E_NUMBER"); code(parse_size("0x10"),"E_SIZE");
    code(parse_aspect("1:0"),"E_ASPECT"); code(parse_rate_pct("6"),"E_RATE");
}
#[test]
fn keys_and_raw_values() {
    let key = parse_key("@Reiter+0.2s:1~spring(170, 26)").unwrap();
    assert_eq!(key.v,"1"); assert!(matches!(key.ease,Some(Ease::Spring { .. })));
    assert_eq!(key.t,KeyTime::At(parse_anchor("Reiter+0.2s").unwrap()));
    assert_eq!(parse_key("50%:1").unwrap().t,KeyTime::Pct(0.5));
    assert_eq!(parse_key("12f:0").unwrap().t,KeyTime::Dur(Dur::Frames(12.0)));
    assert_eq!(parse_key("0:fn(a:b~c):tail~linear").unwrap().v,"fn(a:b~c):tail");
    assert_eq!(parse_key("0:a~b~hold").unwrap().v,"a~b");
    for s in ["0s", "0s:", "0s:fn(", "0s:x)"] { code(parse_key(s),"E_KEY"); }
    assert_eq!(as_raw(&agentfx_script::ast::Value::Str("0.4s".into())),"0.4s");
}
#[test]
fn suggestions() {
    assert_eq!(suggest("bounse", &["spring","bounce"]),Some("bounce"));
    assert_eq!(suggest("BOUNCE", &["bounce"]),Some("bounce"));
    assert_eq!(suggest("bnouce", &["bounce"]),Some("bounce"));
    assert_eq!(suggest("CA", &["abc"]),Some("abc")); // unrestricted transposition plus insertion
    assert_eq!(suggest("out", &["cut","outer"]),Some("outer"));
    assert_eq!(suggest("something-unrelated", &["bounce"]),None);
    assert_eq!(suggest("", &["bounce"]),None);
}
