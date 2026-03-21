/// Cleanup modes for the LLM post-processing step.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum CleanupMode {
    Standard,
    Markdown,
}

/// Standard mode system prompt (Cycle 3 winner — ROUGE-L 0.845, chrF 0.845).
#[allow(dead_code)]
pub const STANDARD_PROMPT: &str = "\
Fix this speech transcript. Remove all verbal fillers and hesitations such as uh and um. \
Remove crutch phrases such as basically and you know. Fix grammar and misheard words. \
Remove false starts where the speaker restarts a sentence. When the speaker changes \
their mind, keep only the final version. If the speaker lists items by number, format \
as a numbered list. Preserve all meaningful content — do not summarize or shorten. \
Output only the cleaned text.";

/// Markdown mode system prompt.
#[allow(dead_code)]
pub const MARKDOWN_PROMPT: &str = "\
You are a transcript-to-markdown converter. Take the raw speech transcript and convert \
it into well-structured Markdown.\n\n\
Rules:\n\
1. Remove filler words (uh, um, like, you know)\n\
2. Fix grammar and misheard words\n\
3. Organize content with headings (## for main topics)\n\
4. Use bullet lists for items and details\n\
5. Use numbered lists for sequential items or action items\n\
6. Use bold for emphasis on key terms\n\
7. Keep all information — do not summarize\n\n\
Output ONLY the markdown, no commentary.";

/// Build the full chat prompt for Qwen3 using the ChatML template.
#[allow(dead_code)]
pub fn build_chat_prompt(mode: CleanupMode, transcript: &str) -> String {
    let system = match mode {
        CleanupMode::Standard => STANDARD_PROMPT,
        CleanupMode::Markdown => MARKDOWN_PROMPT,
    };

    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{transcript}<|im_end|>\n<|im_start|>assistant\n"
    )
}

/// Get the system prompt for a given mode.
#[allow(dead_code)]
pub fn system_prompt(mode: CleanupMode) -> &'static str {
    match mode {
        CleanupMode::Standard => STANDARD_PROMPT,
        CleanupMode::Markdown => MARKDOWN_PROMPT,
    }
}
