use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use zip::write::{SimpleFileOptions, ZipWriter};

const MIB: u64 = 1024 * 1024;
const SAFETY_BYTES: u64 = MIB;
const TEXT_UNIT: &str =
    "OfficeCLI HCD streaming stress paragraph: 中文、emoji😀、table-like values 1234567890. ";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let target = PathBuf::from(arguments.next().ok_or("missing output DOCX path")?);
    let target_mib: u64 = arguments
        .next()
        .map(|value| value.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(2040);
    if arguments.next().is_some() || !(16..=2040).contains(&target_mib) {
        return Err("usage: generate_stress_docx <output.docx> [16..2040 MiB]".into());
    }
    if target.exists() {
        return Err(format!("target already exists: {}", target.display()).into());
    }

    let file = BufWriter::with_capacity(256 * 1024, File::create(&target)?);
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", stored)?;
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)?;
    zip.start_file("_rels/.rels", stored)?;
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#)?;
    zip.start_file("word/document.xml", deflated)?;
    let prefix = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#;
    let suffix = br#"<w:sectPr/></w:body></w:document>"#;
    zip.write_all(prefix)?;

    let document_budget = target_mib * MIB - SAFETY_BYTES;
    let mut logical_bytes = prefix.len() as u64;
    let mut paragraph = 0u64;
    while logical_bytes + suffix.len() as u64 + 8192 <= document_budget {
        paragraph += 1;
        let mut text = String::with_capacity(4600);
        for slot in 0..48u64 {
            text.push_str(TEXT_UNIT);
            if slot % 6 == 0 {
                let token = mix(paragraph.wrapping_mul(97).wrapping_add(slot));
                write!(&mut text, " {token:016x} ")?;
            }
        }
        let xml = format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
        if logical_bytes + xml.len() as u64 + suffix.len() as u64 > document_budget {
            break;
        }
        zip.write_all(xml.as_bytes())?;
        logical_bytes += xml.len() as u64;
        if paragraph.is_multiple_of(50_000) {
            eprintln!("generated {} MiB of document.xml", logical_bytes / MIB);
        }
    }
    zip.write_all(suffix)?;
    logical_bytes += suffix.len() as u64;
    zip.finish()?.flush()?;
    eprintln!(
        "created {}: document.xml={} MiB, paragraphs={paragraph}",
        target.display(),
        logical_bytes / MIB
    );
    Ok(())
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
