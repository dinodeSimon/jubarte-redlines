// SPDX-FileCopyrightText: 2026 Jandira Technologies, LLC
//
// SPDX-License-Identifier: AGPL-3.0-only

//! A PDF's title and language, as Word's Save as PDF writes them: `/Title`
//! from the file's core properties, shown in the title bar
//! (`/DisplayDocTitle`), and `/Lang` from the document's default language,
//! else the theme fonts' language. A caller may give either instead.

use std::io::{Cursor, Write};

use jubarte::convert::{PdfInfo, PdfOptions, docx_to_pdf_with, docx_to_pdf_with_info};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// A one-paragraph package with these core properties, styles and settings
/// (each left out when `None`).
fn docx(core_title: Option<&str>, default_lang: Option<&str>, theme_lang: Option<&str>) -> Vec<u8> {
    let mut parts: Vec<(&str, String)> = Vec::new();
    let mut overrides = String::from(
        "<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    );
    let mut package_rels = String::from(
        "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>",
    );
    let mut document_rels = String::new();
    if let Some(title) = core_title {
        overrides.push_str("<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>");
        package_rels.push_str("<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>");
        parts.push((
            "docProps/core.xml",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{title}</dc:title></cp:coreProperties>"
            ),
        ));
    }
    let lang = default_lang
        .map(|l| format!("<w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val=\"{l}\"/></w:rPr></w:rPrDefault></w:docDefaults>"))
        .unwrap_or_default();
    overrides.push_str("<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>");
    document_rels.push_str("<Relationship Id=\"rIdS\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>");
    parts.push((
        "word/styles.xml",
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:styles xmlns:w=\"{W}\">{lang}</w:styles>"
        ),
    ));
    if let Some(theme) = theme_lang {
        overrides.push_str("<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>");
        document_rels.push_str("<Relationship Id=\"rIdT\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>");
        parts.push((
            "word/settings.xml",
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:settings xmlns:w=\"{W}\"><w:themeFontLang w:val=\"{theme}\"/></w:settings>"),
        ));
    }
    parts.push((
        "[Content_Types].xml",
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>{overrides}</Types>"
        ),
    ));
    parts.push((
        "_rels/.rels",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{package_rels}</Relationships>"),
    ));
    parts.push((
        "word/_rels/document.xml.rels",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{document_rels}</Relationships>"),
    ));
    parts.push((
        "word/document.xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document xmlns:w=\"{W}\"><w:body><w:p><w:r><w:t>Quote</w:t></w:r></w:p></w:body></w:document>"),
    ));
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn text(pdf: &[u8]) -> String {
    String::from_utf8_lossy(pdf).into_owned()
}

#[test]
fn writes_the_core_title_and_the_default_language() {
    let pdf = text(
        &docx_to_pdf_with(
            &docx(Some("Quote Q-1042"), Some("en-AU"), Some("de-DE")),
            PdfOptions::default(),
        )
        .unwrap(),
    );
    assert!(pdf.contains("/Type /Catalog /Pages 2 0 R /Lang (en-AU) /ViewerPreferences << /DisplayDocTitle true >> >>"), "{pdf}");
    assert!(
        pdf.contains("<< /Title (Quote Q-1042) /Producer (jubarte) /Creator (jubarte) >>"),
        "{pdf}"
    );
}

#[test]
fn takes_the_theme_fonts_language_when_the_defaults_name_none() {
    let pdf =
        text(&docx_to_pdf_with(&docx(None, None, Some("de-DE")), PdfOptions::default()).unwrap());
    assert!(
        pdf.contains("<< /Type /Catalog /Pages 2 0 R /Lang (de-DE) >>"),
        "{pdf}"
    );
    // No title: no /Title, and the viewer shows the file's name.
    assert!(
        pdf.contains("<< /Producer (jubarte) /Creator (jubarte) >>"),
        "{pdf}"
    );
    assert!(!pdf.contains("DisplayDocTitle"));
}

#[test]
fn writes_neither_where_the_file_says_neither() {
    let pdf = text(&docx_to_pdf_with(&docx(None, None, None), PdfOptions::default()).unwrap());
    assert!(pdf.contains("<< /Type /Catalog /Pages 2 0 R >>"), "{pdf}");
    assert!(!pdf.contains("/Lang") && !pdf.contains("/Title"));
}

#[test]
fn a_given_title_and_language_win_over_the_files() {
    let given = PdfInfo {
        title: Some("Devis Q-1042 — Harbour".into()),
        lang: Some("fr-FR".into()),
    };
    let pdf = text(
        &docx_to_pdf_with_info(
            &docx(Some("Quote"), Some("en-AU"), None),
            PdfOptions::default(),
            &given,
        )
        .unwrap(),
    );
    assert!(pdf.contains("/Lang (fr-FR)"), "{pdf}");
    // Not ASCII: a UTF-16BE string, as every other PDF text string the writer makes.
    assert!(pdf.contains("/Title <FEFF"), "{pdf}");
    assert!(!pdf.contains("(Quote)") && !pdf.contains("en-AU"));
    // Only the title given: the file's language stays.
    let title_only = PdfInfo {
        title: Some("Given".into()),
        lang: None,
    };
    let pdf = text(
        &docx_to_pdf_with_info(
            &docx(Some("Quote"), Some("en-AU"), None),
            PdfOptions::default(),
            &title_only,
        )
        .unwrap(),
    );
    assert!(
        pdf.contains("/Lang (en-AU)") && pdf.contains("/Title (Given)"),
        "{pdf}"
    );
}

#[test]
fn a_language_that_is_not_a_tag_is_left_out_and_a_title_cannot_break_its_string() {
    let pdf = text(
        &docx_to_pdf_with(
            &docx(Some("a) /JS (alert"), Some("en) /OpenAction (x"), None),
            PdfOptions::default(),
        )
        .unwrap(),
    );
    assert!(!pdf.contains("/Lang"), "{pdf}");
    assert!(
        !pdf.contains("/OpenAction") && !pdf.contains(") /JS ("),
        "{pdf}"
    );
    assert!(pdf.contains("/Title (a\\) /JS \\(alert)"), "{pdf}");
}
