use gen_lsp_types::{
    ActiveParameter, ParameterInformation, ParameterInformationLabel, SignatureHelp,
    SignatureInformation,
};
use rg_analysis::SignatureHelp as AnalysisSignatureHelp;
use rg_lsp_proto::SignatureHelpClientCapabilities;

pub(crate) fn signature_help(
    help: AnalysisSignatureHelp,
    client: SignatureHelpClientCapabilities,
) -> SignatureHelp {
    let parameters = help
        .parameters
        .into_iter()
        .map(|range| {
            let text = &help.label[range.clone()];
            let label = if client.label_offset_support {
                let start = help.label[..range.start].encode_utf16().count() as u32;
                let end = start + text.encode_utf16().count() as u32;
                ParameterInformationLabel::Tuple((start, end))
            } else {
                ParameterInformationLabel::String(text.to_owned())
            };
            ParameterInformation {
                label,
                documentation: None,
            }
        })
        .collect();
    let active_parameter = help
        .active_parameter
        .map(|index| ActiveParameter::Int(index as u32))
        .or_else(|| {
            client
                .no_active_parameter_support
                .then_some(ActiveParameter::Null)
        });
    SignatureHelp {
        signatures: vec![SignatureInformation {
            label: help.label,
            parameters: Some(parameters),
            ..Default::default()
        }],
        active_signature: Some(0),
        active_parameter,
    }
}
