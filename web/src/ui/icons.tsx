// Minimal stroke icon set (16px grid).
import type { JSX } from 'preact';

type P = { size?: number } & JSX.SVGAttributes<SVGSVGElement>;

const I = (d: JSX.Element, { size = 16, ...rest }: P) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {...rest}>
    {d}
  </svg>
);

export const Play = (p: P) => I(<path d="M5 3.2v9.6c0 .5.5.8.9.5l7.4-4.8c.4-.3.4-.8 0-1L5.9 2.7c-.4-.3-.9 0-.9.5z" fill="currentColor" stroke="none" />, p);
export const Pause = (p: P) => I(<g fill="currentColor" stroke="none"><rect x="4" y="3" width="3" height="10" rx="1" /><rect x="9" y="3" width="3" height="10" rx="1" /></g>, p);
export const Undo = (p: P) => I(<><path d="M5 4L2 7l3 3" /><path d="M2 7h7.5a4 4 0 010 8H7" /></>, p);
export const Redo = (p: P) => I(<><path d="M11 4l3 3-3 3" /><path d="M14 7H6.5a4 4 0 000 8H9" /></>, p);
export const Sun = (p: P) => I(<><circle cx="8" cy="8" r="3" /><path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M3.4 12.6l1-1M11.6 4.4l1-1" /></>, p);
export const Moon = (p: P) => I(<path d="M13.5 9.5A5.5 5.5 0 016.5 2.5a5.5 5.5 0 107 7z" />, p);
export const Download = (p: P) => I(<><path d="M8 2v8.5M4.5 7.5L8 11l3.5-3.5" /><path d="M2.5 13.5h11" /></>, p);
export const Upload = (p: P) => I(<><path d="M8 11V2.5M4.5 6L8 2.5 11.5 6" /><path d="M2.5 13.5h11" /></>, p);
export const Music = (p: P) => I(<><path d="M6 12V3.5l7-1.5V10" /><circle cx="4.3" cy="12" r="1.8" /><circle cx="11.3" cy="10" r="1.8" /></>, p);
export const Image = (p: P) => I(<><rect x="2" y="3" width="12" height="10" rx="2" /><circle cx="6" cy="6.5" r="1.2" /><path d="M2.5 12l3.5-3.5 3 3 2-2 2.5 2.5" /></>, p);
export const Code = (p: P) => I(<><path d="M5.5 4.5L2 8l3.5 3.5M10.5 4.5L14 8l-3.5 3.5" /></>, p);
export const Grid = (p: P) => I(<><rect x="2" y="2" width="5" height="5" rx="1.3" /><rect x="9" y="2" width="5" height="5" rx="1.3" /><rect x="2" y="9" width="5" height="5" rx="1.3" /><rect x="9" y="9" width="5" height="5" rx="1.3" /></>, p);
export const Search = (p: P) => I(<><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5L14 14" /></>, p);
export const X = (p: P) => I(<path d="M4 4l8 8M12 4l-8 8" />, p);
export const Check = (p: P) => I(<path d="M3 8.5l3 3 7-7" />, p);
export const Spark = (p: P) => I(<path d="M8 1.5l1.6 4.2 4.4 1.6-4.4 1.6L8 13.5 6.4 8.9 2 7.3l4.4-1.6z" />, p);
export const Film = (p: P) => I(<><rect x="2" y="2.5" width="12" height="11" rx="2" /><path d="M5 2.5v11M11 2.5v11M2 6h3M2 10h3M11 6h3M11 10h3" /></>, p);
export const Trash = (p: P) => I(<><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5c0 .5.4.9.9.9h4c.5 0 .9-.4.9-.9l.6-8.5" /></>, p);
export const Plus = (p: P) => I(<path d="M8 3v10M3 8h10" />, p);
export const Diamond = (p: P) => I(<path d="M8 2.5L13.5 8 8 13.5 2.5 8z" />, p);
export const Chevron = (p: P) => I(<path d="M6 4l4 4-4 4" />, p);
export const Eye = (p: P) => I(<><path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z" /><circle cx="8" cy="8" r="2" /></>, p);
export const Wand = (p: P) => I(<><path d="M3 13l7.5-7.5M9.5 3.5l3 3" /><path d="M12 1.5v2M14.5 4h-2M12.5 9v1.5M13.3 9.8h-1.6" /></>, p);
export const Ext = (p: P) => I(<><path d="M9.5 2.5h4v4M13.5 2.5L7.5 8.5" /><path d="M12 9.5v3a1 1 0 01-1 1H3.5a1 1 0 01-1-1V5a1 1 0 011-1h3" /></>, p);
export const Layers = (p: P) => I(<><path d="M8 2l6 3.2-6 3.2-6-3.2z" /><path d="M2 8.2l6 3.2 6-3.2" /></>, p);
export const Text = (p: P) => I(<path d="M3 4V2.8h10V4M8 2.8v10.5M6 13.3h4" />, p);
export const Shape = (p: P) => I(<><circle cx="6" cy="6" r="3.5" /><rect x="7.5" y="7.5" width="6" height="6" rx="1" /></>, p);
export const Alert = (p: P) => I(<><path d="M8 2l6.5 11.5h-13z" /><path d="M8 6.5v3M8 11.5v.2" /></>, p);
