use std::ops::Range;

/// The declaration shown at a call site and the parameter under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    /// UTF-8 byte ranges in the label for the parameters the caller writes as arguments.
    /// For `client.send(payload)`, the label includes the receiver and the first range points
    /// to the declared `payload` parameter.
    pub parameters: Vec<Range<usize>>,
    /// Index into `parameters`, when the current argument has a corresponding parameter.
    pub active_parameter: Option<usize>,
}
