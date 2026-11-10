import { useRef } from 'preact/hooks';
import { fmtClock } from '../model/time';
import { Pause, Play } from '../ui/icons';

interface Props {
  time: number;
  duration: number;
  playing: boolean;
  onPlay: (p: boolean) => void;
  onSeek: (t: number) => void;
  right?: preact.ComponentChildren;
  fps?: number;
}

export function Transport({ time, duration, playing, onPlay, onSeek, right, fps = 30 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const seekAt = (e: PointerEvent) => {
    const r = ref.current!.getBoundingClientRect();
    const t = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * duration;
    onSeek(Math.round(t * fps) / fps);
  };
  const pct = duration ? (time / duration) * 100 : 0;
  const ticks = [];
  for (let s = 1; s < duration; s++) ticks.push(<div key={s} class="tick" style={{ left: `${(s / duration) * 100}%` }} />);
  return (
    <div class="transport">
      <button class="playbtn" onClick={() => onPlay(!playing)} aria-label={playing ? 'Pause' : 'Play'} data-testid="play" title="Play / pause (Space)">
        {playing ? <Pause /> : <Play />}
      </button>
      <div class="time" data-testid="time">
        {fmtClock(time)} <span class="faint">/ {fmtClock(duration)}</span>
      </div>
      <div
        class="scrub"
        ref={ref}
        role="slider"
        aria-label="Playhead"
        aria-valuemin={0}
        aria-valuemax={duration}
        aria-valuenow={time}
        tabIndex={0}
        onPointerDown={(e) => {
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          onPlay(false);
          seekAt(e);
        }}
        onPointerMove={(e) => {
          if ((e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) seekAt(e);
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowRight') onSeek(Math.min(duration, time + (e.shiftKey ? 1 : 1 / fps)));
          if (e.key === 'ArrowLeft') onSeek(Math.max(0, time - (e.shiftKey ? 1 : 1 / fps)));
        }}
      >
        <div class="track">
          <div class="fill" style={{ width: `${pct}%` }} />
        </div>
        {ticks}
        <div class="knob" style={{ left: `${pct}%` }} />
      </div>
      <div class="row">{right}</div>
    </div>
  );
}
