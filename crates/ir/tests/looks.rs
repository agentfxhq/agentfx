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
