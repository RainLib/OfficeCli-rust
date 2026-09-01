use crate::content_stream::{
    parse_page_content_stream, parse_page_content_stream_text_only_bounded,
    parse_page_content_stream_with_images_bounded, ParsedContentStream,
};
use handler_common::HandlerError;
use lopdf::{Dictionary, Document as LopdfDocument, Object, ObjectId, Stream};
use std::io::Read;
use std::path::Path;

/// Limits applied before lopdf builds its in-memory PDF object graph.
#[derive(Clone, Copy, Debug)]
pub struct PdfStructuralLimits {
    pub maximum_source_bytes: usize,
    pub maximum_dictionary_bytes: usize,
    pub maximum_encoded_stream_bytes: usize,
    pub maximum_decoded_stream_bytes: usize,
    pub maximum_total_decoded_bytes: usize,
    pub maximum_structural_streams: usize,
    pub maximum_xref_entries: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StructuralStreamKind {
    Object,
    CrossReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StructuralFilter {
    Flate,
    Ascii85,
    Lzw,
    Unsupported,
}

#[derive(Debug, Default)]
struct StreamDictionaryInfo {
    kind: Option<StructuralStreamKind>,
    filters: Option<Vec<StructuralFilter>>,
    length: Option<usize>,
    has_decode_parameters: bool,
    object_count: Option<i64>,
    xref_size: Option<i64>,
    xref_widths: Option<Vec<i64>>,
    xref_index: Option<Vec<i64>>,
}

/// Preflight the structural streams that lopdf eagerly decompresses while
/// loading a document. The source buffer is released before `PdfReader::open`
/// is called, so this prevents `/ObjStm` and `/XRef` decompression bombs from
/// reaching lopdf without retaining a second copy for the lifetime of the PDF.
pub fn preflight_structural_streams(
    path: impl AsRef<Path>,
    limits: PdfStructuralLimits,
) -> Result<(), HandlerError> {
    let path = path.as_ref();
    let source_size = std::fs::metadata(path)
        .map_err(|error| HandlerError::OpenError(format!("failed to inspect PDF: {error}")))?
        .len();
    if source_size > limits.maximum_source_bytes as u64 {
        return Err(resource_limit("PDF source", limits.maximum_source_bytes));
    }
    let source = std::fs::read(path)
        .map_err(|error| HandlerError::OpenError(format!("failed to read PDF: {error}")))?;
    if source.len() > limits.maximum_source_bytes {
        return Err(resource_limit("PDF source", limits.maximum_source_bytes));
    }
    preflight_structural_stream_bytes(&source, limits)
}

fn preflight_structural_stream_bytes(
    source: &[u8],
    limits: PdfStructuralLimits,
) -> Result<(), HandlerError> {
    let mut lexer = PdfLexer::new(source, 0, source.len());
    let mut dictionary_starts = Vec::new();
    let mut last_dictionary = None;
    let mut structural_streams = 0usize;
    let mut total_decoded = 0usize;

    while let Some(token) = lexer.next_token()? {
        match token.kind {
            PdfTokenKind::DictionaryStart => {
                dictionary_starts.push(token.start);
                last_dictionary = None;
            }
            PdfTokenKind::DictionaryEnd => {
                let start = dictionary_starts.pop().ok_or_else(|| {
                    invalid_pdf("PDF dictionary closes without a matching opening delimiter")
                })?;
                last_dictionary = Some((start, token.end));
            }
            PdfTokenKind::Word
                if last_dictionary.is_some() && token_equals(source, token, b"stream") =>
            {
                let (dictionary_start, dictionary_end) = last_dictionary.take().unwrap();
                let dictionary_bytes = dictionary_end.saturating_sub(dictionary_start);
                if dictionary_bytes > limits.maximum_dictionary_bytes {
                    return Err(resource_limit(
                        "PDF stream dictionary",
                        limits.maximum_dictionary_bytes,
                    ));
                }
                let info = inspect_stream_dictionary(source, dictionary_start, dictionary_end)?;
                let content_start = stream_content_start(source, token.end)?;
                let (content_end, marker_end) = stream_content_end(
                    source,
                    content_start,
                    info.length,
                    limits.maximum_source_bytes,
                )?;

                if let Some(kind) = info.kind {
                    structural_streams = structural_streams.saturating_add(1);
                    if structural_streams > limits.maximum_structural_streams {
                        return Err(resource_limit(
                            "PDF structural stream count",
                            limits.maximum_structural_streams,
                        ));
                    }
                    validate_structural_dictionary(kind, &info, limits.maximum_xref_entries)?;
                    let encoded = &source[content_start..content_end];
                    if encoded.len() > limits.maximum_encoded_stream_bytes {
                        return Err(resource_limit(
                            "PDF structural encoded stream",
                            limits.maximum_encoded_stream_bytes,
                        ));
                    }
                    let decoded = decode_structural_stream_bounded(
                        encoded,
                        &info,
                        limits.maximum_decoded_stream_bytes,
                    )?;
                    total_decoded = total_decoded.saturating_add(decoded);
                    if total_decoded > limits.maximum_total_decoded_bytes {
                        return Err(resource_limit(
                            "PDF structural streams total",
                            limits.maximum_total_decoded_bytes,
                        ));
                    }
                }

                lexer.set_position(marker_end);
                dictionary_starts.clear();
                last_dictionary = None;
            }
            _ => last_dictionary = None,
        }
    }
    if !dictionary_starts.is_empty() {
        return Err(invalid_pdf("PDF contains an unterminated dictionary"));
    }
    Ok(())
}

fn validate_structural_dictionary(
    kind: StructuralStreamKind,
    info: &StreamDictionaryInfo,
    maximum_xref_entries: usize,
) -> Result<(), HandlerError> {
    match kind {
        StructuralStreamKind::Object => {
            if info.object_count.is_some_and(|count| {
                count < 0
                    || usize::try_from(count).map_or(true, |count| count > maximum_xref_entries)
            }) {
                return Err(resource_limit(
                    "PDF object-stream object count",
                    maximum_xref_entries,
                ));
            }
        }
        StructuralStreamKind::CrossReference => {
            let size = info
                .xref_size
                .ok_or_else(|| invalid_pdf("PDF cross-reference stream has no direct /Size"))?;
            if size < 0 || usize::try_from(size).map_or(true, |size| size > maximum_xref_entries) {
                return Err(resource_limit(
                    "PDF cross-reference entry count",
                    maximum_xref_entries,
                ));
            }
            let widths = info
                .xref_widths
                .as_ref()
                .ok_or_else(|| invalid_pdf("PDF cross-reference stream has no direct /W array"))?;
            if widths.len() < 3 || widths.iter().any(|width| !(0..=8).contains(width)) {
                return Err(invalid_pdf(
                    "PDF cross-reference /W widths must contain at least three values in 0..=8",
                ));
            }
            if let Some(index) = &info.xref_index {
                if index.len() % 2 != 0 || index.iter().any(|value| *value < 0) {
                    return Err(invalid_pdf(
                        "PDF cross-reference /Index must contain non-negative start/count pairs",
                    ));
                }
                let entries = index
                    .chunks_exact(2)
                    .try_fold(0usize, |total, pair| {
                        usize::try_from(pair[1])
                            .ok()
                            .and_then(|count| total.checked_add(count))
                    })
                    .ok_or_else(|| {
                        resource_limit(
                            "PDF cross-reference /Index entry count",
                            maximum_xref_entries,
                        )
                    })?;
                if entries > maximum_xref_entries {
                    return Err(resource_limit(
                        "PDF cross-reference /Index entry count",
                        maximum_xref_entries,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn decode_structural_stream_bounded(
    encoded: &[u8],
    info: &StreamDictionaryInfo,
    maximum: usize,
) -> Result<usize, HandlerError> {
    let filters = info.filters.as_deref().unwrap_or_default();
    if filters.contains(&StructuralFilter::Unsupported) {
        return Err(HandlerError::UnsupportedMode(
            "PDF structural stream uses a filter unsupported by the bounded preflight".to_string(),
        ));
    }
    if info.has_decode_parameters && filters.contains(&StructuralFilter::Lzw) {
        return Err(HandlerError::UnsupportedMode(
            "PDF structural LZW stream with /DecodeParms is not supported by the bounded preflight"
                .to_string(),
        ));
    }
    let mut dictionary = Dictionary::new();
    if filters.len() == 1 {
        dictionary.set(
            "Filter",
            Object::Name(filter_name(filters[0]).unwrap().to_vec()),
        );
    } else if !filters.is_empty() {
        dictionary.set(
            "Filter",
            Object::Array(
                filters
                    .iter()
                    .map(|filter| Object::Name(filter_name(*filter).unwrap().to_vec()))
                    .collect(),
            ),
        );
    }
    let stream = Stream::new(dictionary, encoded.to_vec());
    decode_stream_bounded(&stream, maximum, "PDF structural stream").map(|bytes| bytes.len())
}

fn filter_name(filter: StructuralFilter) -> Option<&'static [u8]> {
    match filter {
        StructuralFilter::Flate => Some(b"FlateDecode"),
        StructuralFilter::Ascii85 => Some(b"ASCII85Decode"),
        StructuralFilter::Lzw => Some(b"LZWDecode"),
        StructuralFilter::Unsupported => None,
    }
}

fn inspect_stream_dictionary(
    source: &[u8],
    start: usize,
    end: usize,
) -> Result<StreamDictionaryInfo, HandlerError> {
    let mut lexer = PdfLexer::new(source, start, end);
    let opening = lexer
        .next_token()?
        .ok_or_else(|| invalid_pdf("empty PDF stream dictionary"))?;
    if opening.kind != PdfTokenKind::DictionaryStart {
        return Err(invalid_pdf("PDF stream dictionary does not start with <<"));
    }
    let mut info = StreamDictionaryInfo::default();
    while let Some(key) = lexer.next_token()? {
        if key.kind == PdfTokenKind::DictionaryEnd {
            break;
        }
        if key.kind != PdfTokenKind::Name {
            continue;
        }
        let value = lexer
            .next_token()?
            .ok_or_else(|| invalid_pdf("PDF stream dictionary key has no value"))?;
        if name_equals(source, key, b"Type") {
            info.kind = if value.kind == PdfTokenKind::Name && name_equals(source, value, b"ObjStm")
            {
                Some(StructuralStreamKind::Object)
            } else if value.kind == PdfTokenKind::Name && name_equals(source, value, b"XRef") {
                Some(StructuralStreamKind::CrossReference)
            } else {
                None
            };
            skip_composite_value(&mut lexer, value)?;
        } else if name_equals(source, key, b"Filter") {
            info.filters = Some(parse_filters(source, &mut lexer, value)?);
        } else if name_equals(source, key, b"Length") {
            info.length = parse_direct_usize(source, &mut lexer, value)?;
        } else if name_equals(source, key, b"DecodeParms") {
            info.has_decode_parameters =
                value.kind != PdfTokenKind::Word || !token_equals(source, value, b"null");
            skip_composite_value(&mut lexer, value)?;
        } else if name_equals(source, key, b"N") {
            info.object_count = parse_direct_i64(source, &mut lexer, value)?;
        } else if name_equals(source, key, b"Size") {
            info.xref_size = parse_direct_i64(source, &mut lexer, value)?;
        } else if name_equals(source, key, b"W") {
            info.xref_widths = parse_integer_array(source, &mut lexer, value)?;
        } else if name_equals(source, key, b"Index") {
            info.xref_index = parse_integer_array(source, &mut lexer, value)?;
        } else {
            skip_composite_value(&mut lexer, value)?;
        }
    }
    Ok(info)
}

fn parse_filters(
    source: &[u8],
    lexer: &mut PdfLexer<'_>,
    first: PdfToken,
) -> Result<Vec<StructuralFilter>, HandlerError> {
    if first.kind == PdfTokenKind::Name {
        return Ok(vec![parse_filter(source, first)?]);
    }
    if first.kind != PdfTokenKind::ArrayStart {
        skip_composite_value(lexer, first)?;
        return Ok(vec![StructuralFilter::Unsupported]);
    }
    let mut filters = Vec::new();
    while let Some(token) = lexer.next_token()? {
        match token.kind {
            PdfTokenKind::ArrayEnd => return Ok(filters),
            PdfTokenKind::Name => filters.push(parse_filter(source, token)?),
            _ => {
                filters.push(StructuralFilter::Unsupported);
                skip_composite_value(lexer, token)?;
            }
        }
    }
    Err(invalid_pdf("unterminated PDF structural /Filter array"))
}

fn parse_filter(source: &[u8], token: PdfToken) -> Result<StructuralFilter, HandlerError> {
    if name_equals(source, token, b"FlateDecode") || name_equals(source, token, b"Fl") {
        Ok(StructuralFilter::Flate)
    } else if name_equals(source, token, b"ASCII85Decode") || name_equals(source, token, b"A85") {
        Ok(StructuralFilter::Ascii85)
    } else if name_equals(source, token, b"LZWDecode") || name_equals(source, token, b"LZW") {
        Ok(StructuralFilter::Lzw)
    } else {
        Ok(StructuralFilter::Unsupported)
    }
}

fn parse_direct_usize(
    source: &[u8],
    lexer: &mut PdfLexer<'_>,
    first: PdfToken,
) -> Result<Option<usize>, HandlerError> {
    let value = parse_direct_i64(source, lexer, first)?;
    match value {
        Some(value) if value < 0 => Err(invalid_pdf("PDF stream /Length is negative")),
        Some(value) => usize::try_from(value)
            .map(Some)
            .map_err(|_| invalid_pdf("PDF stream /Length does not fit in memory")),
        None => Ok(None),
    }
}

fn parse_direct_i64(
    source: &[u8],
    lexer: &mut PdfLexer<'_>,
    first: PdfToken,
) -> Result<Option<i64>, HandlerError> {
    if first.kind != PdfTokenKind::Word {
        skip_composite_value(lexer, first)?;
        return Ok(None);
    }
    let Some(value) = token_i64(source, first) else {
        return Ok(None);
    };
    let mut lookahead = lexer.clone();
    let second = lookahead.next_token()?;
    let reference = match second {
        Some(second)
            if second.kind == PdfTokenKind::Word && token_i64(source, second).is_some() =>
        {
            lookahead.next_token()?.is_some_and(|token| {
                token.kind == PdfTokenKind::Word && token_equals(source, token, b"R")
            })
        }
        _ => false,
    };
    if reference {
        *lexer = lookahead;
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn parse_integer_array(
    source: &[u8],
    lexer: &mut PdfLexer<'_>,
    first: PdfToken,
) -> Result<Option<Vec<i64>>, HandlerError> {
    if first.kind != PdfTokenKind::ArrayStart {
        skip_composite_value(lexer, first)?;
        return Ok(None);
    }
    let mut values = Vec::new();
    while let Some(token) = lexer.next_token()? {
        match token.kind {
            PdfTokenKind::ArrayEnd => return Ok(Some(values)),
            PdfTokenKind::Word => {
                let value = token_i64(source, token).ok_or_else(|| {
                    invalid_pdf("PDF structural integer array contains a non-integer")
                })?;
                values.push(value);
            }
            _ => {
                return Err(invalid_pdf(
                    "PDF structural integer array contains a non-integer",
                ))
            }
        }
    }
    Err(invalid_pdf("unterminated PDF structural integer array"))
}

fn skip_composite_value(lexer: &mut PdfLexer<'_>, first: PdfToken) -> Result<(), HandlerError> {
    let mut arrays = usize::from(first.kind == PdfTokenKind::ArrayStart);
    let mut dictionaries = usize::from(first.kind == PdfTokenKind::DictionaryStart);
    while arrays > 0 || dictionaries > 0 {
        let token = lexer
            .next_token()?
            .ok_or_else(|| invalid_pdf("unterminated composite PDF dictionary value"))?;
        match token.kind {
            PdfTokenKind::ArrayStart => arrays = arrays.saturating_add(1),
            PdfTokenKind::ArrayEnd => {
                arrays = arrays
                    .checked_sub(1)
                    .ok_or_else(|| invalid_pdf("unbalanced PDF array"))?
            }
            PdfTokenKind::DictionaryStart => dictionaries = dictionaries.saturating_add(1),
            PdfTokenKind::DictionaryEnd => {
                dictionaries = dictionaries
                    .checked_sub(1)
                    .ok_or_else(|| invalid_pdf("unbalanced PDF dictionary"))?
            }
            _ => {}
        }
    }
    Ok(())
}

fn stream_content_start(source: &[u8], mut position: usize) -> Result<usize, HandlerError> {
    while source
        .get(position)
        .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
    {
        position += 1;
    }
    match source.get(position..) {
        Some([b'\r', b'\n', ..]) => Ok(position + 2),
        Some([b'\r' | b'\n', ..]) => Ok(position + 1),
        _ => Err(invalid_pdf(
            "PDF stream keyword is not followed by an end-of-line",
        )),
    }
}

fn stream_content_end(
    source: &[u8],
    content_start: usize,
    direct_length: Option<usize>,
    maximum_scan: usize,
) -> Result<(usize, usize), HandlerError> {
    if let Some(length) = direct_length {
        let content_end = content_start
            .checked_add(length)
            .filter(|end| *end <= source.len())
            .ok_or_else(|| invalid_pdf("PDF stream /Length extends beyond the source"))?;
        let mut marker = content_end;
        if source.get(marker..marker + 2) == Some(b"\r\n") {
            marker += 2;
        } else if source
            .get(marker)
            .is_some_and(|byte| *byte == b'\r' || *byte == b'\n')
        {
            marker += 1;
        }
        if source.get(marker..marker.saturating_add(9)) != Some(b"endstream") {
            return Err(invalid_pdf(
                "PDF stream /Length does not end at the endstream marker",
            ));
        }
        return Ok((content_end, marker + 9));
    }

    let scan_end = content_start.saturating_add(maximum_scan).min(source.len());
    let relative = find_subslice(&source[content_start..scan_end], b"endstream")
        .ok_or_else(|| invalid_pdf("PDF stream has no bounded endstream marker"))?;
    let mut content_end = content_start + relative;
    while content_end > content_start && matches!(source[content_end - 1], b'\r' | b'\n') {
        content_end -= 1;
    }
    Ok((content_end, content_start + relative + 9))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn token_i64(source: &[u8], token: PdfToken) -> Option<i64> {
    std::str::from_utf8(&source[token.start..token.end])
        .ok()?
        .parse()
        .ok()
}

fn token_equals(source: &[u8], token: PdfToken, expected: &[u8]) -> bool {
    source.get(token.start..token.end) == Some(expected)
}

fn name_equals(source: &[u8], token: PdfToken, expected: &[u8]) -> bool {
    if token.kind != PdfTokenKind::Name || source.get(token.start) != Some(&b'/') {
        return false;
    }
    let raw = &source[token.start + 1..token.end];
    let mut expected_index = 0usize;
    let mut index = 0usize;
    while index < raw.len() {
        let byte = if raw[index] == b'#' && index + 2 < raw.len() {
            let Some(high) = hex_value(raw[index + 1]) else {
                return false;
            };
            let Some(low) = hex_value(raw[index + 2]) else {
                return false;
            };
            index += 3;
            high * 16 + low
        } else {
            let byte = raw[index];
            index += 1;
            byte
        };
        if expected.get(expected_index) != Some(&byte) {
            return false;
        }
        expected_index += 1;
    }
    expected_index == expected.len()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid_pdf(message: &str) -> HandlerError {
    HandlerError::OpenError(format!("invalid PDF structure: {message}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PdfTokenKind {
    DictionaryStart,
    DictionaryEnd,
    ArrayStart,
    ArrayEnd,
    Name,
    Word,
    Atom,
}

#[derive(Clone, Copy, Debug)]
struct PdfToken {
    kind: PdfTokenKind,
    start: usize,
    end: usize,
}

#[derive(Clone)]
struct PdfLexer<'a> {
    source: &'a [u8],
    position: usize,
    end: usize,
}

impl<'a> PdfLexer<'a> {
    fn new(source: &'a [u8], position: usize, end: usize) -> Self {
        Self {
            source,
            position,
            end,
        }
    }

    fn set_position(&mut self, position: usize) {
        self.position = position.min(self.end);
    }

    fn next_token(&mut self) -> Result<Option<PdfToken>, HandlerError> {
        self.skip_space_and_comments();
        if self.position >= self.end {
            return Ok(None);
        }
        let start = self.position;
        let byte = self.source[start];
        let token = match byte {
            b'<' if self.source.get(start + 1) == Some(&b'<') => {
                self.position += 2;
                PdfToken {
                    kind: PdfTokenKind::DictionaryStart,
                    start,
                    end: self.position,
                }
            }
            b'>' if self.source.get(start + 1) == Some(&b'>') => {
                self.position += 2;
                PdfToken {
                    kind: PdfTokenKind::DictionaryEnd,
                    start,
                    end: self.position,
                }
            }
            b'[' => self.single(PdfTokenKind::ArrayStart, start),
            b']' => self.single(PdfTokenKind::ArrayEnd, start),
            b'/' => {
                self.position += 1;
                while self.position < self.end
                    && !is_pdf_space(self.source[self.position])
                    && !is_pdf_delimiter(self.source[self.position])
                {
                    self.position += 1;
                }
                PdfToken {
                    kind: PdfTokenKind::Name,
                    start,
                    end: self.position,
                }
            }
            b'(' => {
                self.skip_literal_string()?;
                PdfToken {
                    kind: PdfTokenKind::Atom,
                    start,
                    end: self.position,
                }
            }
            b'<' => {
                self.position += 1;
                while self.position < self.end && self.source[self.position] != b'>' {
                    self.position += 1;
                }
                if self.position >= self.end {
                    return Err(invalid_pdf("unterminated PDF hexadecimal string"));
                }
                self.position += 1;
                PdfToken {
                    kind: PdfTokenKind::Atom,
                    start,
                    end: self.position,
                }
            }
            b'{' | b'}' => self.single(PdfTokenKind::Atom, start),
            _ => {
                while self.position < self.end
                    && !is_pdf_space(self.source[self.position])
                    && !is_pdf_delimiter(self.source[self.position])
                {
                    self.position += 1;
                }
                if self.position == start {
                    self.position += 1;
                }
                PdfToken {
                    kind: PdfTokenKind::Word,
                    start,
                    end: self.position,
                }
            }
        };
        Ok(Some(token))
    }

    fn single(&mut self, kind: PdfTokenKind, start: usize) -> PdfToken {
        self.position += 1;
        PdfToken {
            kind,
            start,
            end: self.position,
        }
    }

    fn skip_space_and_comments(&mut self) {
        loop {
            while self.position < self.end && is_pdf_space(self.source[self.position]) {
                self.position += 1;
            }
            if self.position >= self.end || self.source[self.position] != b'%' {
                break;
            }
            while self.position < self.end && !matches!(self.source[self.position], b'\r' | b'\n') {
                self.position += 1;
            }
        }
    }

    fn skip_literal_string(&mut self) -> Result<(), HandlerError> {
        let mut depth = 0usize;
        while self.position < self.end {
            let byte = self.source[self.position];
            self.position += 1;
            match byte {
                b'\\' => {
                    if self.position < self.end {
                        if self.source[self.position] == b'\r'
                            && self.source.get(self.position + 1) == Some(&b'\n')
                        {
                            self.position += 2;
                        } else {
                            self.position += 1;
                        }
                    }
                }
                b'(' => depth = depth.saturating_add(1),
                b')' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid_pdf("unbalanced PDF literal string"))?;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(invalid_pdf("unterminated PDF literal string"))
    }
}

fn is_pdf_space(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// PDF document reader using lopdf.
pub struct PdfReader {
    doc: LopdfDocument,
    page_count: usize,
    page_ids: Vec<ObjectId>,
    file_path: String,
}

impl PdfReader {
    /// Open a PDF document.
    pub fn open(path: &str) -> Result<Self, HandlerError> {
        let doc = LopdfDocument::load(path)
            .map_err(|e| HandlerError::OpenError(format!("failed to open PDF: {}", e)))?;
        let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
        let page_count = page_ids.len();
        Ok(Self {
            doc,
            page_count,
            page_ids,
            file_path: path.to_string(),
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }
    pub fn document(&self) -> &LopdfDocument {
        &self.doc
    }
    pub fn document_mut(&mut self) -> &mut LopdfDocument {
        &mut self.doc
    }
    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    /// Recount pages from the document (e.g. after deleting a page).
    pub fn recount_pages(&mut self) {
        self.page_ids = self.doc.get_pages().into_values().collect();
        self.page_count = self.page_ids.len();
    }

    /// Create a fallback reader with an empty document (used when re-loading fails).
    pub fn fallback(page_count: usize, file_path: &str) -> Self {
        Self {
            doc: LopdfDocument::new(),
            page_count,
            page_ids: Vec::new(),
            file_path: file_path.to_string(),
        }
    }

    /// Extract text from all pages.
    pub fn extract_all_text(&self) -> String {
        let mut full_text = String::new();
        for i in 1..=self.page_count {
            if let Some(page_text) = self.extract_page_text(i) {
                if !full_text.is_empty() {
                    full_text.push('\n');
                }
                full_text.push_str(&page_text);
            }
        }
        full_text
    }

    /// Extract text from a specific page.
    pub fn extract_page_text(&self, page_num: usize) -> Option<String> {
        let parsed = self.parse_page_text_blocks(page_num)?;
        let mut text = String::new();
        for block in &parsed.text_blocks {
            if !text.is_empty() {
                // Check if this block is on a new line relative to the previous one
                // (different y coordinate indicates a new line)
                text.push('\n');
            }
            text.push_str(&block.text);
        }
        Some(text)
    }

    /// Parse a page's content stream into structured text blocks with bbox info.
    pub fn parse_page_text_blocks(&self, page_num: usize) -> Option<ParsedContentStream> {
        let page_id = *self.page_ids.get(page_num.checked_sub(1)?)?;
        let content = self.doc.get_page_content(page_id).ok()?;
        parse_page_content_stream(&content, page_id, &self.doc).ok()
    }

    /// Parse one page for HCD without materializing image payloads and with
    /// hard limits on decoded page-content and auxiliary font streams.
    pub fn parse_page_text_blocks_bounded(
        &self,
        page_num: usize,
        max_page_content_bytes: usize,
        max_aux_stream_bytes: usize,
    ) -> Result<ParsedContentStream, HandlerError> {
        let index = page_num.checked_sub(1).ok_or_else(|| {
            HandlerError::InvalidArgument("PDF page numbers are 1-based".to_string())
        })?;
        let page_id = *self.page_ids.get(index).ok_or_else(|| {
            HandlerError::InvalidArgument(format!("PDF page {page_num} does not exist"))
        })?;
        let content = page_content_bounded(&self.doc, page_id, max_page_content_bytes)?;
        parse_page_content_stream_text_only_bounded(
            &content,
            page_id,
            &self.doc,
            max_aux_stream_bytes,
        )
    }

    /// Parse one page with bounded content/font streams and a bounded image
    /// presentation layer. Unlike `parse_page_text_blocks`, this is suitable
    /// for HCD's page-at-a-time importer.
    pub fn parse_page_text_blocks_with_images_bounded(
        &self,
        page_num: usize,
        max_page_content_bytes: usize,
        max_aux_stream_bytes: usize,
        max_image_payload_bytes: usize,
    ) -> Result<ParsedContentStream, HandlerError> {
        let index = page_num.checked_sub(1).ok_or_else(|| {
            HandlerError::InvalidArgument("PDF page numbers are 1-based".to_string())
        })?;
        let page_id = *self.page_ids.get(index).ok_or_else(|| {
            HandlerError::InvalidArgument(format!("PDF page {page_num} does not exist"))
        })?;
        let content = page_content_bounded(&self.doc, page_id, max_page_content_bytes)?;
        parse_page_content_stream_with_images_bounded(
            &content,
            page_id,
            &self.doc,
            max_aux_stream_bytes,
            max_image_payload_bytes,
        )
    }
}

fn page_content_bounded(
    doc: &LopdfDocument,
    page_id: ObjectId,
    maximum: usize,
) -> Result<Vec<u8>, HandlerError> {
    let mut content = Vec::with_capacity(maximum.min(64 * 1024));
    for object_id in doc.get_page_contents(page_id) {
        let stream = doc
            .get_object(object_id)
            .and_then(lopdf::Object::as_stream)
            .map_err(|error| {
                HandlerError::OpenError(format!(
                    "failed to read PDF page content stream {object_id:?}: {error}"
                ))
            })?;
        let remaining = maximum.saturating_sub(content.len());
        let decoded = decode_stream_bounded(stream, remaining, "PDF page content")?;
        if content.len().saturating_add(decoded.len()) > maximum {
            return Err(resource_limit("PDF page content", maximum));
        }
        content.extend_from_slice(&decoded);
    }
    Ok(content)
}

pub(crate) fn decode_stream_bounded(
    stream: &Stream,
    maximum: usize,
    context: &str,
) -> Result<Vec<u8>, HandlerError> {
    let filters = if stream.dict.has(b"Filter") {
        stream.filters().map_err(|error| {
            HandlerError::OpenError(format!("invalid {context} filter: {error}"))
        })?
    } else {
        Vec::new()
    };
    if filters.is_empty() {
        if stream.content.len() > maximum {
            return Err(resource_limit(context, maximum));
        }
        return Ok(stream.content.clone());
    }
    if has_predictor(stream) {
        return Err(HandlerError::UnsupportedMode(format!(
            "{context} uses a PNG/TIFF predictor that is not supported by the bounded HCD PDF decoder"
        )));
    }

    let mut current = Vec::new();
    for (index, filter) in filters.iter().enumerate() {
        let input = if index == 0 {
            stream.content.as_slice()
        } else {
            current.as_slice()
        };
        current = match filter.as_str() {
            "FlateDecode" | "Fl" => decode_flate_bounded(input, maximum, context)?,
            "ASCII85Decode" | "A85" => decode_ascii85_bounded(input, maximum, context)?,
            "LZWDecode" | "LZW" => decode_lzw_bounded(input, maximum, context, stream)?,
            other => {
                return Err(HandlerError::UnsupportedMode(format!(
                    "{context} uses unsupported bounded-decoder filter {other}"
                )))
            }
        };
    }
    Ok(current)
}

fn decode_lzw_bounded(
    input: &[u8],
    maximum: usize,
    context: &str,
    stream: &Stream,
) -> Result<Vec<u8>, HandlerError> {
    use weezl::decode::Decoder;
    use weezl::{BitOrder, LzwStatus};

    let early_change = stream
        .dict
        .get(b"DecodeParms")
        .and_then(lopdf::Object::as_dict)
        .ok()
        .and_then(|parameters| parameters.get(b"EarlyChange").ok())
        .and_then(|value| value.as_i64().ok())
        .is_none_or(|value| value != 0);
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    let mut output = Vec::with_capacity(input.len().saturating_mul(2).min(maximum));
    let mut offset = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let result = decoder.decode_bytes(&input[offset..], &mut buffer);
        append_bounded(
            &mut output,
            &buffer[..result.consumed_out],
            maximum,
            context,
        )?;
        offset = offset.saturating_add(result.consumed_in);
        match result.status {
            Ok(LzwStatus::Done) => break,
            Ok(LzwStatus::Ok) if offset < input.len() => {}
            Ok(LzwStatus::Ok | LzwStatus::NoProgress) if offset == input.len() => break,
            Ok(LzwStatus::NoProgress) => {
                return Err(HandlerError::OpenError(format!(
                    "failed to decode {context} LZW stream: decoder made no progress"
                )))
            }
            Err(error) => {
                return Err(HandlerError::OpenError(format!(
                    "failed to decode {context} LZW stream: {error}"
                )))
            }
            Ok(LzwStatus::Ok) => break,
        }
    }
    Ok(output)
}

fn decode_flate_bounded(
    input: &[u8],
    maximum: usize,
    context: &str,
) -> Result<Vec<u8>, HandlerError> {
    let decoder = flate2::read::ZlibDecoder::new(input);
    let mut limited = decoder.take(maximum.saturating_add(1) as u64);
    let mut output = Vec::with_capacity(input.len().saturating_mul(2).min(maximum));
    limited.read_to_end(&mut output).map_err(|error| {
        HandlerError::OpenError(format!("failed to decode {context} Flate stream: {error}"))
    })?;
    if output.len() > maximum {
        return Err(resource_limit(context, maximum));
    }
    Ok(output)
}

fn decode_ascii85_bounded(
    input: &[u8],
    maximum: usize,
    context: &str,
) -> Result<Vec<u8>, HandlerError> {
    let mut output = Vec::with_capacity(input.len().min(maximum));
    let mut buffer: u32 = 0;
    let mut count = 0usize;
    for &byte in input {
        if byte == b'z' && count == 0 {
            append_bounded(&mut output, &[0, 0, 0, 0], maximum, context)?;
            continue;
        }
        if byte.is_ascii_whitespace() || byte == b'~' || byte == b'>' {
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            break;
        }
        buffer = buffer
            .saturating_mul(85)
            .saturating_add((byte - b'!') as u32);
        count += 1;
        if count == 5 {
            append_bounded(&mut output, &buffer.to_be_bytes(), maximum, context)?;
            buffer = 0;
            count = 0;
        }
    }
    if count > 0 {
        for _ in count..5 {
            buffer = buffer.saturating_mul(85).saturating_add(84);
        }
        append_bounded(
            &mut output,
            &buffer.to_be_bytes()[..count - 1],
            maximum,
            context,
        )?;
    }
    Ok(output)
}

fn append_bounded(
    output: &mut Vec<u8>,
    bytes: &[u8],
    maximum: usize,
    context: &str,
) -> Result<(), HandlerError> {
    if output.len().saturating_add(bytes.len()) > maximum {
        return Err(resource_limit(context, maximum));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn has_predictor(stream: &Stream) -> bool {
    let Ok(parameters) = stream.dict.get(b"DecodeParms") else {
        return false;
    };
    if let Ok(dictionary) = parameters.as_dict() {
        return dictionary
            .get(b"Predictor")
            .and_then(lopdf::Object::as_i64)
            .is_ok_and(|value| value > 1);
    }
    // Filter-aligned DecodeParms arrays require filter-specific processing;
    // reject them in the bounded path instead of risking unbounded fallback.
    parameters.as_array().is_ok()
}

fn resource_limit(context: &str, maximum: usize) -> HandlerError {
    HandlerError::InvalidArgument(format!(
        "resource limit exceeded: {context} is larger than {maximum} decoded bytes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use lopdf::{dictionary, Document, Object, Stream};
    use std::io::Write;

    fn write_pdf(path: &std::path::Path, page_content: Vec<u8>) -> ObjectId {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut content_stream = Stream::new(dictionary! {}, page_content);
        content_stream.compress().unwrap();
        let content_id = document.add_object(content_stream);
        let resources_id = document.add_object(dictionary! {});
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
        content_id
    }

    fn structural_stream(kind: &str, dictionary: &str, decoded_size: usize) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&vec![b'x'; decoded_size]).unwrap();
        let encoded = encoder.finish().unwrap();
        let header = format!(
            "%PDF-1.7\n1 0 obj\n<< /Type /{kind} {dictionary} /Filter /FlateDecode /Length {} >>\nstream\n",
            encoded.len()
        );
        let mut source = header.into_bytes();
        source.extend_from_slice(&encoded);
        source.extend_from_slice(b"\nendstream\nendobj\n");
        source
    }

    fn small_structural_limits() -> PdfStructuralLimits {
        PdfStructuralLimits {
            maximum_source_bytes: 2 * 1024 * 1024,
            maximum_dictionary_bytes: 64 * 1024,
            maximum_encoded_stream_bytes: 1024 * 1024,
            maximum_decoded_stream_bytes: 64 * 1024,
            maximum_total_decoded_bytes: 96 * 1024,
            maximum_structural_streams: 8,
            maximum_xref_entries: 1000,
        }
    }

    #[test]
    fn open_keeps_streams_compressed_and_bounded_page_decode_rejects_a_bomb() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bomb.pdf");
        let content_id = write_pdf(&path, vec![b'x'; 2 * 1024 * 1024]);

        let reader = PdfReader::open(path.to_str().unwrap()).unwrap();
        let stream = reader
            .document()
            .get_object(content_id)
            .and_then(Object::as_stream)
            .unwrap();
        assert!(
            stream.dict.has(b"Filter"),
            "open must not eagerly decompress"
        );

        let error = reader
            .parse_page_text_blocks_bounded(1, 64 * 1024, 64 * 1024)
            .unwrap_err();
        assert!(error.to_string().contains("resource limit exceeded"));
    }

    #[test]
    fn bounded_page_decode_parses_normal_text_without_image_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("normal.pdf");
        write_pdf(
            &path,
            b"BT /F1 12 Tf 72 720 Td (Hello bounded PDF) Tj ET".to_vec(),
        );

        let reader = PdfReader::open(path.to_str().unwrap()).unwrap();
        let parsed = reader
            .parse_page_text_blocks_bounded(1, 64 * 1024, 64 * 1024)
            .unwrap();
        assert_eq!(parsed.text_blocks.len(), 1);
        assert_eq!(parsed.text_blocks[0].text, "Hello bounded PDF");
        assert!(parsed.image_map.is_empty());
    }

    #[test]
    fn structural_preflight_rejects_object_stream_bombs_and_decodes_escaped_names() {
        let source = structural_stream("Obj#53tm", "/N 0 /First 0", 128 * 1024);
        let error =
            preflight_structural_stream_bytes(&source, small_structural_limits()).unwrap_err();
        assert!(error.to_string().contains("resource limit exceeded"));
        assert!(error.to_string().contains("PDF structural stream"));
    }

    #[test]
    fn structural_preflight_rejects_cross_reference_stream_bombs() {
        let source = structural_stream("XRef", "/Size 1 /W [1 2 1] /Index [0 1]", 128 * 1024);
        let error =
            preflight_structural_stream_bytes(&source, small_structural_limits()).unwrap_err();
        assert!(error.to_string().contains("resource limit exceeded"));
    }

    #[test]
    fn structural_preflight_rejects_xref_width_allocation_bombs() {
        let source = structural_stream("XRef", "/Size 1 /W [1 999999999 1]", 8);
        let error =
            preflight_structural_stream_bytes(&source, small_structural_limits()).unwrap_err();
        assert!(error.to_string().contains("cross-reference /W widths"));
    }

    #[test]
    fn structural_preflight_does_not_decode_ordinary_page_streams() {
        let source = structural_stream("Page", "", 128 * 1024);
        preflight_structural_stream_bytes(&source, small_structural_limits()).unwrap();

        let source = source
            .windows(b"FlateDecode".len())
            .position(|window| window == b"FlateDecode")
            .map(|position| {
                let mut source = source;
                source.splice(
                    position..position + b"FlateDecode".len(),
                    b"DCTDecode".iter().copied(),
                );
                source
            })
            .unwrap();
        preflight_structural_stream_bytes(&source, small_structural_limits()).unwrap();
    }
}
