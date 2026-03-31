/// Cleanup modes for the LLM post-processing step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CleanupMode {
    Standard,
    Markdown,
}
