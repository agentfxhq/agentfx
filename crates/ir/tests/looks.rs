use agentfx_ir::{Effect, Scene, validate::validate};
use serde_json::{json, Value};
fn scene(effect:Value)->Scene {
    serde_json::from_value(json!({"version":1,"width":64,"height":64,"fps":30,"frames":2,"nodes":[{"name":"root","effects":[effect]}]})).unwrap()
}
#[test]
fn looks_validate_roundtrip_and_tracks() {
    let valid=[json!({"type":"rgbsplit","dx":4,"dy":0,"amount":1}),json!({"type":"glitch","amount":1,"rate":8,"block":12}),json!({"type":"vhs","amount":1}),json!({"type":"dither","levels":2,"mode":"bayer"}),json!({"type":"ascii","cell":8,"charset":" .#@","color":"mono","bg":[0,0,0,1]}),json!({"type":"glass","strength":12,"blur":1,"dispersion":2,"thickness":1.2,"highlight":0.2,"rim":0.3,"tint":[0.8,0.9,1,0.1]}),json!({"type":"optics","streak":0.5,"bokeh":1,"distortion":0.1,"aberration":0.3,"vignette":0.4,"halation":0.2,"grain":0.1,"seed":7})];
    for value in valid {
        let mut s=scene(value); assert!(validate(&s,std::path::Path::new(".")).is_ok());
        let e=&s.nodes[0].effects[0]; let encoded=serde_json::to_string(e).unwrap();assert_eq!(*e,serde_json::from_str::<Effect>(&encoded).unwrap());
        for field in e.look_fields() {assert!(e.look_field(field).is_finite());}
        s.nodes[0].effects[0]=match e {Effect::Rgbsplit{..}=>Effect::Rgbsplit{dx:0.0,dy:0.0,amount:2.0,mode:"bad".into()},Effect::Glitch{..}=>Effect::Glitch{amount:1.0,seed:0,rate:-1.0,block:0.0},Effect::Vhs{..}=>Effect::Vhs{amount:f64::NAN,seed:0},Effect::Dither{..}=>Effect::Dither{levels:1.0,mode:"bad".into(),size:3,palette:Some(vec![])},Effect::Glass{..}=>Effect::Glass{strength:f64::NAN,blur:0.0,dispersion:0.0,thickness:1.0,highlight:0.0,rim:0.0,tint:None},Effect::Optics{..}=>Effect::Optics{streak:f64::NAN,bokeh:0.0,distortion:0.0,aberration:0.0,vignette:0.0,halation:0.0,grain:0.0,seed:0},_=>Effect::Ascii{cell:0.0,charset:"A".into(),color:"bad".into(),bg:[2.0;4]}};
        assert!(validate(&s,std::path::Path::new(".")).is_err());
    }
}

#[test]
fn warps_validate_roundtrip_and_tracks() {
    let ok = [
        json!({"type":"kaleido","center":[32,32]}),
        json!({"type":"kaleido","segments":12,"mirror":false,"center":[10,20],"angle":30,"zoom":1.5,"amount":0.5,"rect":[0,0,64,64]}),
    ];
    let bad = [
        json!({"type":"kaleido","segments":1,"center":[32,32]}),
        json!({"type":"kaleido","center":[32,32],"zoom":0}),
        json!({"type":"kaleido","center":[32,32],"amount":2}),
        json!({"type":"kaleido","center":[32,32],"rect":[0,0,0,10]}),
    ];
    for (valid, list) in [(true, &ok[..]), (false, &bad[..])] {
        for v in list {
            let s = scene(v.clone());
            assert_eq!(validate(&s, std::path::Path::new(".")).is_ok(), valid, "{v}");
            let e = &s.nodes[0].effects[0];
            assert_eq!(*e, serde_json::from_str::<Effect>(&serde_json::to_string(e).unwrap()).unwrap());
            assert!(e.is_warp() && e.is_look());
        }
    }
    let k = &scene(ok[1].clone()).nodes[0].effects[0];
    assert_eq!(k.look_fields(), &["amount", "angle", "zoom", "cx", "cy"]);
    assert_eq!([k.look_field("cx"), k.look_field("cy"), k.look_field("zoom")], [10.0, 20.0, 1.5]);
    assert!(!k.time_dependent());
    // effects.0.angle tracks validate
    let mut s = scene(ok[0].clone());
    s.tracks = vec![serde_json::from_value(json!({"node":0,"property":"effects.0.angle","delay":0,"duration":1,"fill":"both","easing":"linear","keyframes":[{"offset":0,"value":{"number":0}},{"offset":1,"value":{"number":90}}]})).unwrap()];
    assert!(validate(&s, std::path::Path::new(".")).is_ok());
}

#[test]
fn tunnel_validates_and_depends_on_time_only_with_speed() {
    let ok = [
        json!({"type":"tunnel","radius":120,"center":[32,32]}),
        json!({"type":"tunnel","amount":0.3,"depth":2,"speed":1.5,"twist":20,"angle":10,"radius":90,"fog":0.2,"fog_color":[0.1,0,0.2,1],"center":[32,32],"map":"perspective","rect":[0,0,64,64]}),
    ];
    let bad = [
        json!({"type":"tunnel","radius":0.5,"center":[32,32]}),
        json!({"type":"tunnel","radius":100,"center":[32,32],"amount":1.5}),
        json!({"type":"tunnel","radius":100,"center":[32,32],"fog":-1}),
        json!({"type":"tunnel","radius":100,"center":[32,32],"map":"sphere"}),
        json!({"type":"tunnel","radius":100,"center":[32,32],"fog_color":[2,0,0,1]}),
    ];
    for (valid, list) in [(true, &ok[..]), (false, &bad[..])] {
        for v in list {
            let s = scene(v.clone());
            assert_eq!(validate(&s, std::path::Path::new(".")).is_ok(), valid, "{v}");
            let e = &s.nodes[0].effects[0];
            assert_eq!(*e, serde_json::from_str::<Effect>(&serde_json::to_string(e).unwrap()).unwrap());
        }
    }
    let a = &scene(ok[0].clone()).nodes[0].effects[0];
    let b = &scene(ok[1].clone()).nodes[0].effects[0];
    assert!(!a.time_dependent() && b.time_dependent(), "speed x time");
    assert_eq!(b.look_fields(), &["amount", "depth", "twist", "angle", "radius", "fog"]);
    assert_eq!(b.look_field("radius"), 90.0);
    assert!(serde_json::from_value::<Effect>(json!({"type":"tunnel","center":[0,0]})).is_err(), "radius is required");
}
