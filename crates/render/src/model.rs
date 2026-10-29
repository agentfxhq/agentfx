//! HTML-frontend model: CSS animation tracks bound to DOM elements.
//! Value/easing/transform types come from the IR crate.
pub use agentfx_ir::{Affine, Easing, FillMode, Keyframe, Sampled, StepPos, TransformFn, Value};

/// Index of an element in document order, identical to the index in
/// `document.querySelectorAll('*')` in Chrome (html=0, head=1, ...).
pub type ElemId = usize;

/// Which pipeline stage a property change invalidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Composite,
    Paint,
    Layout,
}

/// One animated property of one CSS animation (or tracks.json entry) on one element.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub elem: ElemId,
    /// CSS property name (`transform`, `opacity`, `width`, ...) or `text`
    /// (tracks.json typed text: value = number of visible chars, floor()).
    pub property: String,
    pub stage: Stage,
    /// Position in the element's `animation-name` list; later wins when several
    /// animations set the same property. tracks.json tracks use 1000 + index.
    pub order: usize,
    /// Name of the @keyframes rule (or `tracks.json#i`), for reporting.
    pub source: String,
    /// Seconds.
    pub delay: f64,
    /// Seconds, > 0.
    pub duration: f64,
    pub fill: FillMode,
    pub easing: Easing,
    /// Sorted by offset; first offset 0.0, last 1.0.
    pub keyframes: Vec<Keyframe>,
}

impl Track {
    /// The same animation as an IR track on IR node `node`.
    pub fn to_ir(&self, node: usize) -> agentfx_ir::Track {
        agentfx_ir::Track {
            node,
            property: self.property.clone(),
            order: self.order,
            label: self.source.clone(),
            delay: self.delay,
            duration: self.duration,
            fill: self.fill,
            easing: self.easing,
            keyframes: self.keyframes.clone(),
            stagger: None,
        }
    }
}

/// Text track from the scene's tracks.json (visible text = first floor(v) chars).
#[derive(Clone, Debug)]
pub struct TextTrack {
    pub elem: ElemId,
    pub text: String,
}

/// Output of the CSS front end (css.rs).
#[derive(Clone, Debug)]
pub struct Compiled {
    /// Lower-case tag name per ElemId.
    pub tags: Vec<String>,
    /// Parent element per ElemId (None for <html>).
    pub parent: Vec<Option<ElemId>>,
    pub tracks: Vec<Track>,
    pub texts: Vec<TextTrack>,
    /// Seconds, from <meta name="reel-duration" content="8">.
    pub duration: f64,
    /// From <meta name="reel-fps" content="30"> (default 30).
    pub fps: f64,
    /// Scene written by the track recorder (`<meta name="reel-frames">` present).
    pub recorded: bool,
}
