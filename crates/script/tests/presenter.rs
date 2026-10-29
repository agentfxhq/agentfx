use agentfx_script::{parse, timing, lower, ir::{Ir, Ch}, diag::{Diag, Severity}};
use std::{collections::HashMap, path::Path};
fn compile(src:&str)->(Ir,Vec<Diag>) {
    let (doc,errs)=parse::parse(src);assert!(errs.is_empty(),"{errs:?}");
    let tts:HashMap<_,_>=timing::tts_requests(&doc).iter().map(|r|(r.key.clone(),timing::estimate(r))).collect();
    let (tl,mut d)=timing::solve(&doc,&tts,None);
    let c=lower::compile(&doc,&tl,None,Path::new("."));d.extend(c.diags);(c.ir,d)
}
const SRC:&str="video size=1920x1080 fps=30 captions=off\nscene demo dur=8s\n  text #target \"Hello\" x=750 y=450 w=180 h=80\n  presenter #m skin=paper x=70% y=55% h=70% seed=42 look=#target\n  say \"Hello [[world]].\"\n    @world gesture #m point #target ; mood #m happy\n  @4s gesture #m wave\n";
#[test] fn seed_determinism_and_rig() {
    let (a,d)=compile(SRC);assert!(!d.iter().any(|d|d.severity==Severity::Error),"{d:?}");
    let (b,_)=compile(SRC);assert_eq!(serde_json::to_value(&a).unwrap(),serde_json::to_value(&b).unwrap());
    let (c,_)=compile(&SRC.replace("seed=42","seed=43"));assert_ne!(serde_json::to_value(&a).unwrap(),serde_json::to_value(&c).unwrap());
    let mut bones=vec![];a.root.walk(&mut |l|bones.push(l.id.clone()));
    for bone in ["torso~head~eyes","torso~upper_arm_L~forearm_L~hand_L","torso~upper_arm_R~forearm_R~hand_R"] {assert!(bones.iter().any(|n|n.ends_with(bone)),"{bone}");}
}
#[test] fn gesture_tracks_use_anchor_times_and_forearm_lag() {
    let (ir,d)=compile("video captions=off\nscene s dur=6s\n  presenter #m\n  @2s gesture #m wave\n");assert!(!d.iter().any(|d|d.severity==Severity::Error),"{d:?}");
    let upper=ir.tracks.iter().find(|t|t.layer.ends_with("upper_arm_R")&&t.ch==Ch::Rot).unwrap();
    let fore=ir.tracks.iter().find(|t|t.layer.ends_with("forearm_R")&&t.ch==Ch::Rot).unwrap();
    assert!((upper.eval(1.9).n()+8.0).abs()<0.001);
    assert!(upper.keys.iter().any(|k|(k.t-2.32).abs()<0.01));
    assert!(fore.keys.iter().any(|k|(k.t-2.38).abs()<0.01));
    assert!((upper.eval(3.9).n()+8.0).abs()<0.001);
}
#[test] fn mouth_is_exclusive_and_never_switches_under_two_frames() {
    let (ir,_)=compile(SRC);
    let tracks:Vec<_>=ir.tracks.iter().filter(|t|t.layer.contains("~mouth-")&&!t.layer.contains("~rest-")&&t.ch==Ch::Opacity).collect();
    let mut previous=0;let mut since=0;
    for frame in 0..240 {
        let t=frame as f64/30.0;
        let active:Vec<_>=tracks.iter().enumerate().filter(|(_,tr)|tr.eval(t).n()>0.5).collect();
        assert_eq!(active.len(),1,"frame {frame}");
        let state=active[0].0;
        if frame>0 && previous!=state {assert!(frame-since>=2,"frame {frame}");since=frame;}
        previous=state;
    }
}
#[test] fn presenter_diagnostics() {
    for bad in ["presenter #m skin=flta","presenter #m mood=hapy","presenter #m look=#missing","presenter #m\n  @1s gesture #m waev","presenter #m\n  @1s mood #m hapy","presenter #m\n  @1s gesture #m point #missing","presenter #m\n  @1s gesture #m count 7"] {
        let (_,d)=compile(&format!("video captions=off\nscene s dur=4s\n  {bad}\n"));
        assert!(d.iter().any(|d|d.code=="E_PRESENTER"&&!d.hint.is_empty()),"{bad}: {d:?}");
    }
    let (_,d)=compile("scene s dur=1s\n  presenter #m h=20%\n");assert!(d.iter().any(|d|d.code=="W_PRESENTER_SMALL"));
}
#[test] fn all_gestures_and_skins_emit_valid_existing_ir() {
    let mut src=String::from("video captions=off\n");
    for (i,skin) in ["flat","line","paper"].iter().enumerate() {
        src+=&format!("scene s{i} dur=12s\n  text #target \"Target\" x=700 y=400\n  presenter #m skin={skin} x=60% y=55% h=70%\n");
        for (j,gesture) in ["point #target","wave","nod","shrug","think","present #target","count 3"].iter().enumerate() {src+=&format!("  @{}s gesture #m {gesture}\n",j as f64*1.5);}
    }
    let (ir,d)=compile(&src);assert!(!d.iter().any(|d|d.severity==Severity::Error),"{d:?}");
    let out=std::env::temp_dir().join("presenter-test");std::fs::create_dir_all(&out).unwrap();
    let compiled=agentfx_script::direct::compile(&ir,&out,false).unwrap();
    assert!(!compiled.diags.iter().any(|d|d.severity==Severity::Error),"{:?}",compiled.diags);
}
#[test] fn sixty_second_compile_budget() {
    let src="video captions=off\nscene long dur=60s\n  text #target \"Target\" x=700 y=400\n  presenter #m look=#target skin=paper\n  @2s gesture #m point #target\n";
    let start=std::time::Instant::now();let (ir,d)=compile(src);
    let out=std::env::temp_dir().join("presenter-budget");
    let result=agentfx_script::direct::compile(&ir,&out,false).unwrap();
    assert!(!result.diags.iter().any(|d|d.severity==Severity::Error),"{:?}",result.diags);
    assert!(!d.iter().any(|d|d.severity==Severity::Error),"{d:?}");
    let elapsed=start.elapsed();eprintln!("60 second presenter compile: {elapsed:?}");assert!(elapsed.as_secs_f64()<2.0,"{elapsed:?}");
}

#[test] fn native_word_times_drive_the_mouth_and_bones_are_local() {
    let (doc,errs)=parse::parse("video captions=off\nscene s dur=4s\n  presenter #m x=60% y=50%\n  say \"Mama Otto\"\n");
    assert!(errs.is_empty());
    let req=timing::tts_requests(&doc).remove(0);
    let mut tts=HashMap::new();
    tts.insert(req.key,timing::TtsResult {mp3:None,dur:3.0,estimated:false,words:vec![
        timing::TtsWord {text:"Mama".into(),start:0.5,end:1.0},
        timing::TtsWord {text:"Otto".into(),start:2.0,end:2.6},
    ]});
    let (tl,d)=timing::solve(&doc,&tts,None);assert!(d.is_empty(),"{d:?}");
    let c=lower::compile(&doc,&tl,None,Path::new("."));
    let rest=c.ir.tracks.iter().find(|t|t.layer.ends_with("~mouth-rest")&&t.ch==Ch::Opacity).unwrap();
    assert_eq!(rest.eval(0.2).n(),1.0);
    assert_eq!(rest.eval(0.75).n(),0.0);
    assert_eq!(rest.eval(1.5).n(),1.0);
    assert_eq!(rest.eval(2.3).n(),0.0);
    let lint=agentfx_script::lint::static_rules(&doc,&tl,&c.ir);
    assert!(!lint.iter().any(|d|d.code=="E_OUT_OF_FRAME"),"{lint:?}");
}

#[test] fn count_interrupts_an_open_hand() {
    let (ir,d)=compile("video captions=off\nscene s dur=4s\n  presenter #m\n  @0s gesture #m wave\n  @0.3s gesture #m count 3\n");
    assert!(!d.iter().any(|d|d.severity==Severity::Error),"{d:?}");
    let fingers:Vec<_>=ir.tracks.iter().filter(|t|t.layer.contains("hand_R~finger-")&&t.ch==Ch::Opacity).collect();
    assert_eq!(fingers.iter().filter(|t|t.eval(0.8).n()>0.5).count(),3);
}
