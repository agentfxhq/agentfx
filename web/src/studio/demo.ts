// The script Studio opens with when nothing was handed over.
export const DEMO = `// Welcome to AgentFX Studio. Edit the code or drag keyframes in the timeline:
// both stay in sync. Lint problems show up as squiggles with fix hints.
video "Studio demo" size=1280x720 fps=30 captions=off formats=[16:9, 9:16, 1:1]

scene intro dur=3.5s bg=linear(155deg, #10221B 0%, #0B0D14 100%)
  text #kicker "AGENTFX STUDIO" x=50% y=33% anchor=center size=30 weight=700 tracking=0.22 color=#3CBF87 nowrap
  text #title "Motion from text" x=50% y=50% anchor=center size=92 weight=850 color=#FFFFFF nowrap
    when portrait
      set #title size=60
  rect #bar x=50% y=64% w=180 h=8 radius=4 anchor=center fill=#3CBF87
  anim #kicker opacity 0s:0 0.5s:1~out-cubic
  anim #title y 0.2s:56% 0.9s:50%~out-expo
  anim #title opacity 0.2s:0 0.7s:1~out-cubic
  anim #bar sx 0.6s:0 1.4s:1~out-back
  @2.6s anim #title scale 0s:1 0.6s:1.08~inout-sine

scene outro dur=2.5s bg=#0B0D14
  ellipse #dot w=120 h=120 x=50% y=50% anchor=center fill=#F2C14E
  text #bye "Your turn." x=50% y=78% anchor=center size=44 weight=750 color=#FFFFFF nowrap
  anim #dot scale 0s:0 0.6s:1~spring(170,14)
  anim #bye opacity 0.4s:0 0.9s:1~out-cubic
  @1.4s anim #dot x 0s:50% 0.8s:30%~power3.inOut
`;
