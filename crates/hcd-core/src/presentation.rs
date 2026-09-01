use crate::{
    hash_bytes, Bundle, HcdError, HcdManifest, MAX_CHUNK_BYTES, MAX_CONTROL_PART_BYTES,
    MAX_REVISION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

pub const DEFAULT_HTML_PRESENTATION_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Options shared by CLI previews, Java-side inspection downloads and future
/// profile renderers. Rendering is streaming: only one bounded HCD chunk is
/// resident at a time.
#[derive(Debug, Clone)]
pub struct HtmlPresentationOptions {
    pub revision: Option<u64>,
    pub max_output_bytes: u64,
    /// Prefix used to turn canonical `asset://sha256/...` references into
    /// browser-readable URLs. `None` leaves canonical references untouched.
    pub asset_base_href: Option<String>,
    /// Hover-outline state for editable text nodes. The standalone page
    /// exposes it as `body[data-hcd-text-hitboxes=on|off]` for runtime toggles.
    pub text_hitboxes_enabled: bool,
    /// Hover-outline state for image/form visual nodes. This is intentionally
    /// independent from text and exposed as
    /// `body[data-hcd-image-hitboxes=on|off]`.
    pub image_hitboxes_enabled: bool,
}

impl Default for HtmlPresentationOptions {
    fn default() -> Self {
        Self {
            revision: None,
            max_output_bytes: DEFAULT_HTML_PRESENTATION_MAX_BYTES,
            asset_base_href: None,
            text_hitboxes_enabled: true,
            image_hitboxes_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HtmlPresentationReport {
    pub document_id: String,
    pub revision: u64,
    pub profile: String,
    pub chunk_count: usize,
    pub bytes_written: u64,
}

/// Resolve a stable revision view without changing the bundle head.
pub fn manifest_at_revision(
    bundle: &Bundle,
    head: &HcdManifest,
    requested: Option<u64>,
) -> Result<(HcdManifest, u64), HcdError> {
    if head.revision > MAX_REVISION {
        return Err(HcdError::ResourceLimit(format!(
            "manifest revision {} exceeds the maximum {MAX_REVISION}",
            head.revision
        )));
    }
    let revision = requested.unwrap_or(head.revision);
    if revision > head.revision {
        return Err(HcdError::RevisionConflict(format!(
            "requested revision {revision} is ahead of head {}",
            head.revision
        )));
    }
    if revision == head.revision {
        return Ok((head.clone(), revision));
    }
    let record = bundle.revision(revision)?;
    let mut manifest = head.clone();
    manifest.revision = revision;
    manifest.root_hash = record.root_hash;
    manifest.annotation_root_hash = record.annotation_root_hash;
    manifest.index_prefix = record.index_prefix;
    Ok((manifest, revision))
}

/// Materialize canonical HCD fragments into one standalone inspection page.
/// This is a presentation view only; the directory bundle remains the online,
/// randomly accessible authoritative representation.
pub fn render_standalone_html(
    bundle: &Bundle,
    options: &HtmlPresentationOptions,
    output: &mut impl Write,
) -> Result<HtmlPresentationReport, HcdError> {
    let head = bundle.manifest()?;
    let (manifest, revision) = manifest_at_revision(bundle, &head, options.revision)?;
    let asset_hrefs = if options.asset_base_href.is_some() {
        bundle
            .read_asset_index()?
            .into_iter()
            .map(|asset| (asset.hash, asset.href))
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let mut written = 0u64;
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>HCD revision {revision}</title><style>html{{background:#eef1f5}}body{{box-sizing:border-box;max-width:max-content;min-width:min(100%,960px);margin:24px auto;padding:24px;background:#fff;color:#111;box-shadow:0 3px 18px #0002}}.hcd-chunk{{content-visibility:auto;contain-intrinsic-size:auto 800px}}@media(max-width:720px){{body{{margin:0;padding:12px}}}}</style><style>"
        )
        .as_bytes(),
    )?;

    let styles_path = bundle.resolve_href(&manifest.styles_href)?;
    let styles_metadata = std::fs::metadata(&styles_path)?;
    if styles_metadata.len() > MAX_CONTROL_PART_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "HCD stylesheet is {} bytes; maximum is {MAX_CONTROL_PART_BYTES}",
            styles_metadata.len()
        )));
    }
    let styles = std::fs::read(&styles_path)?;
    write_bounded(output, &mut written, options.max_output_bytes, &styles)?;
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        format!(
            "</style></head><body data-hcd-profile=\"{}\" data-hcd-source-format=\"{}\" data-hcd-revision=\"{revision}\" data-hcd-text-hitboxes=\"{}\" data-hcd-image-hitboxes=\"{}\">",
            manifest.profile,
            manifest.source.format,
            if options.text_hitboxes_enabled { "on" } else { "off" },
            if options.image_hitboxes_enabled { "on" } else { "off" }
        )
        .as_bytes(),
    )?;

    let mut expected_sequence = 0usize;
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(&manifest, page_number)?;
        if page.revision != revision || page.page != page_number {
            return Err(HcdError::InvalidBundle(format!(
                "index page {page_number} does not belong to revision {revision}"
            )));
        }
        for descriptor in page.chunks {
            if descriptor.sequence != expected_sequence {
                return Err(HcdError::InvalidBundle(format!(
                    "expected chunk sequence {expected_sequence}, found {}",
                    descriptor.sequence
                )));
            }
            let html = bundle.read_chunk(&descriptor)?;
            if html.len() > MAX_CHUNK_BYTES || html.len() as u64 != descriptor.byte_length {
                return Err(HcdError::InvalidBundle(format!(
                    "chunk {} byte length mismatch",
                    descriptor.chunk_id
                )));
            }
            let actual_hash = hash_bytes(html.as_bytes());
            if actual_hash != descriptor.html_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "chunk {} expected hash {}, actual {actual_hash}",
                    descriptor.chunk_id, descriptor.html_hash
                )));
            }
            let presented_html = if let Some(base_href) = &options.asset_base_href {
                rewrite_asset_references(&html, &asset_hrefs, base_href)
            } else {
                html
            };
            write_bounded(
                output,
                &mut written,
                options.max_output_bytes,
                presented_html.as_bytes(),
            )?;
            expected_sequence += 1;
        }
    }
    if expected_sequence != manifest.chunk_count {
        return Err(HcdError::InvalidBundle(format!(
            "manifest declares {} chunks, materialized {expected_sequence}",
            manifest.chunk_count
        )));
    }
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        b"</body></html>",
    )?;
    Ok(HtmlPresentationReport {
        document_id: manifest.document_id,
        revision,
        profile: manifest.profile,
        chunk_count: expected_sequence,
        bytes_written: written,
    })
}

fn rewrite_asset_references(
    html: &str,
    asset_hrefs: &HashMap<String, String>,
    base_href: &str,
) -> String {
    const PREFIX: &str = "asset://sha256/";
    let mut output = String::with_capacity(html.len());
    let mut remainder = html;
    while let Some(offset) = remainder.find(PREFIX) {
        output.push_str(&remainder[..offset]);
        let candidate = &remainder[offset + PREFIX.len()..];
        let hash_length = candidate
            .bytes()
            .take_while(|byte| byte.is_ascii_hexdigit())
            .count();
        let hash = &candidate[..hash_length];
        if let Some(href) = asset_hrefs.get(hash) {
            output.push_str(base_href);
            output.push_str(href);
            remainder = &candidate[hash_length..];
        } else {
            output.push_str(PREFIX);
            remainder = candidate;
        }
    }
    output.push_str(remainder);
    output
}

fn write_bounded(
    output: &mut impl Write,
    written: &mut u64,
    maximum: u64,
    bytes: &[u8],
) -> Result<(), HcdError> {
    let next = written
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| HcdError::ResourceLimit("HTML byte count overflowed".to_string()))?;
    if next > maximum {
        return Err(HcdError::ResourceLimit(format!(
            "standalone HCD HTML exceeds the {maximum} byte output limit"
        )));
    }
    output.write_all(bytes)?;
    *written = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_checks_the_limit_before_emitting_more_bytes() {
        let mut output = Vec::new();
        let mut written = 1;
        let error = write_bounded(&mut output, &mut written, 1, b"x").unwrap_err();
        assert!(error.to_string().contains("output limit"));
        assert!(output.is_empty());
        assert_eq!(written, 1);
    }

    #[test]
    fn presentation_rewrites_known_asset_uris_only() {
        let hash = "ab".repeat(32);
        let hrefs = HashMap::from([(hash.clone(), format!("assets/sha256/{hash}.png"))]);
        let html =
            format!("<img src=\"asset://sha256/{hash}\"><img src=\"asset://sha256/unknown\">");
        let rewritten = rewrite_asset_references(&html, &hrefs, "../bundle/");
        assert!(rewritten.contains(&format!("src=\"../bundle/assets/sha256/{hash}.png\"")));
        assert!(rewritten.contains("src=\"asset://sha256/unknown\""));
    }
}
