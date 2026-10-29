use agentfx_ir::{Effect, Scene, validate::validate};
use serde_json::{json, Value};
fn scene(effect:Value)->Scene {
    serde_json::from_value(json!({"version":1,"width":64,"height":64,"fps":30,"frames":2,"nodes":[{"name":"root","effects":[effect]}]})).unwrap()
}
#[test]
fn looks_validate_roundtrip_and_tracks() {
    let valid=[json!({"type":"rgbsplit","dx":4,"dy":0,"amount":1}),json!({"type":"glitch","amount":1,"rate":8,"block":12}),json!({"type":"vhs","amount":1}),json!({"type":"dither","levels":2,"mode":"bayer"}),json!({"type":"ascii","cell":8,"charset":" .#@","color":"mono","bg":[0,0,0,1]})];
    for value in valid {
        let mut s=scene(value); assert!(validate(&s,std::path::Path::new(".")).is_ok());
        let e=&s.nodes[0].effects[0]; let encoded=serde_json::to_string(e).unwrap();assert_eq!(*e,serde_json::from_str::<Effect>(&encoded).unwrap());
        for field in e.look_fields() {assert!(e.look_field(field).is_finite());}
        s.nodes[0].effects[0]=match e {Effect::Rgbsplit{..}=>Effect::Rgbsplit{dx:0.0,dy:0.0,amount:2.0,mode:"bad".into()},Effect::Glitch{..}=>Effect::Glitch{amount:1.0,seed:0,rate:-1.0,block:0.0},Effect::Vhs{..}=>Effect::Vhs{amount:f64::NAN,seed:0},Effect::Dither{..}=>Effect::Dither{levels:1.0,mode:"bad".into(),size:3,palette:Some(vec![])},_=>Effect::Ascii{cell:0.0,charset:"A".into(),color:"bad".into(),bg:[2.0;4]}};
        assert!(validate(&s,std::path::Path::new(".")).is_err());
    }
}
