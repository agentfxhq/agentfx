// The live preview runs the compiler's HTML backend (window.__reel: ready/seek/anims) in a sandboxed
// iframe (scripts only, opaque origin). This bridge, appended to the page, drives it by postMessage.

export type FromFrame =
  | { __afx: 1; id: number; type: 'ready'; duration: number; w: number; h: number }
  | { __afx: 1; id: number; type: 'time'; t: number }
  | { __afx: 1; id: number; type: 'seeked'; t: number };

export type ToFrame = { __afx: 1; type: 'seek'; t: number } | { __afx: 1; type: 'play'; t: number } | { __afx: 1; type: 'pause' };

const BRIDGE = `(function(){
var ID=__ID__,R=window.__reel,raf=0,t0=0,from=0,last=0;
function post(m){m.__afx=1;m.id=ID;parent.postMessage(m,'*');}
function stop(){if(raf)cancelAnimationFrame(raf);raf=0;}
function tick(now){var d=R.duration||1,t=from+(now-t0)/1000;if(t>=d){from=0;t0=now;t=0;}
for(var i=0;i<R.anims.length;i++)R.anims[i].currentTime=t*1000;
if(now-last>30){last=now;post({type:'time',t:t});}raf=requestAnimationFrame(tick);}
if(!R||!R.ready){post({type:'ready',duration:0,w:0,h:0});return;}
R.ready.then(function(){return R.seek(0);}).then(function(){post({type:'ready',duration:R.duration,w:R.w,h:R.h});});
addEventListener('message',function(e){var m=e.data;if(!m||m.__afx!==1)return;
if(m.type==='seek'){stop();R.seek(m.t).then(function(){post({type:'seeked',t:m.t});});}
else if(m.type==='play'){stop();R.ready.then(function(){from=m.t||0;t0=performance.now();raf=requestAnimationFrame(tick);});}
else if(m.type==='pause'){stop();}});
})();`;

export function withBridge(html: string, id: number): string {
  const script = `<script>${BRIDGE.replace('__ID__', String(id))}</script>`;
  const i = html.lastIndexOf('</body>');
  return i >= 0 ? html.slice(0, i) + script + html.slice(i) : html + script;
}
