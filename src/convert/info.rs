//! What a PDF says of its document, as Word's Save as PDF writes it: the
//! title (`/Info /Title`, shown in the viewer's title bar with
//! `/ViewerPreferences /DisplayDocTitle`) and the document's language
//! (`/Catalog /Lang`), which a screen reader reads the text in.
//!
//! Word takes the title from the file's core properties (`dc:title`) and the
//! language from the document's default run properties (`w:docDefaults`'s
//! `w:lang`), else the language the theme's fonts are set for
//! (`w:themeFontLang`). A caller may give either instead
//! ([`super::docx_to_pdf_with_info`]).

use super::{descendants_local, element_text, local_name_is, main_rel_part, settings_part};
use crate::namespaces::W;
use crate::opc::PartFs;
use crate::xmllinq::Dom;

/// A PDF's title and language; `None` leaves each out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdfInfo {
    /// `/Info /Title`, and `/DisplayDocTitle` with it.
    pub title: Option<String>,
    /// `/Catalog /Lang`: a BCP 47 language tag such as `en-AU`.
    pub lang: Option<String>,
}

impl PdfInfo {
    /// The title and language a package's own parts give, as Word reads them.
    pub(crate) fn of_package(pkg: &PartFs) -> Self {
        Self {
            title: pkg
                .part_string("docProps/core.xml")
                .and_then(|xml| core_title(&xml)),
            lang: pkg
                .part_string(&main_rel_part(pkg, "styles", "word/styles.xml"))
                .and_then(|xml| default_lang(&xml))
                .or_else(|| {
                    pkg.part_string(&settings_part(pkg))
                        .and_then(|xml| theme_font_lang(&xml))
                }),
        }
    }

    /// Each of `given`'s that is set, else this one's.
    #[must_use]
    pub fn overridden_by(self, given: &Self) -> Self {
        Self {
            title: given.title.clone().or(self.title),
            lang: given.lang.clone().or(self.lang),
        }
    }

    /// The title, cleaned: no control characters, its spaces collapsed, at most
    /// 1,000 characters; none when that leaves nothing.
    pub(crate) fn clean_title(&self) -> Option<String> {
        let words: Vec<&str> = self.title.as_deref()?.split_whitespace().collect();
        let text: String = words
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .take(1000)
            .collect();
        (!text.is_empty()).then_some(text)
    }

    /// The language, when it has the shape of a language tag (letters, digits
    /// and hyphens, starting with a letter, at most 35 characters); none
    /// otherwise, so nothing a file says can break out of the PDF string.
    pub(crate) fn clean_lang(&self) -> Option<&str> {
        let lang = self.lang.as_deref()?.trim();
        let tag = !lang.is_empty()
            && lang.len() <= 35
            && lang.starts_with(|c: char| c.is_ascii_alphabetic())
            && lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        tag.then_some(lang)
    }
}

/// `dc:title` of a core properties part.
fn core_title(xml: &str) -> Option<String> {
    let mut dom = Dom::new();
    let doc = dom.parse_xdocument(xml);
    let root = dom.root(doc)?;
    (0..dom.child_count(root))
        .map(|i| dom.child_at(root, i))
        .find(|&n| local_name_is(&dom, n, "title"))
        .map(|n| element_text(&dom, n))
        .filter(|t| !t.trim().is_empty())
}

/// The `w:val` of the first `w:lang` under `w:docDefaults` of a styles part.
fn default_lang(xml: &str) -> Option<String> {
    let mut dom = Dom::new();
    let doc = dom.parse_xdocument(xml);
    let root = dom.root(doc)?;
    let defaults = descendants_local(&dom, root, "docDefaults")
        .into_iter()
        .next()?;
    descendants_local(&dom, defaults, "lang")
        .into_iter()
        .find_map(|n| dom.attribute(n, &W::val()).map(str::to_string))
        .filter(|v| !v.trim().is_empty())
}

/// The `w:val` of a settings part's `w:themeFontLang`.
fn theme_font_lang(xml: &str) -> Option<String> {
    let mut dom = Dom::new();
    let doc = dom.parse_xdocument(xml);
    let root = dom.root(doc)?;
    descendants_local(&dom, root, "themeFontLang")
        .into_iter()
        .find_map(|n| dom.attribute(n, &W::val()).map(str::to_string))
        .filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{PdfInfo, core_title, default_lang, theme_font_lang};

    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    #[test]
    fn reads_the_core_title() {
        let xml = r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>A</dc:creator><dc:title>Quote for Harbour Freight</dc:title></cp:coreProperties>"#;
        assert_eq!(
            core_title(xml).as_deref(),
            Some("Quote for Harbour Freight")
        );
        let empty = r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>  </dc:title></cp:coreProperties>"#;
        assert_eq!(core_title(empty), None);
    }

    #[test]
    fn reads_the_default_language_then_the_theme_fonts() {
        let styles = format!(
            r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="en-AU" w:eastAsia="ja-JP"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:styleId="x"><w:rPr><w:lang w:val="fr-FR"/></w:rPr></w:style></w:styles>"#
        );
        assert_eq!(default_lang(&styles).as_deref(), Some("en-AU"));
        let none = format!(
            r#"<w:styles xmlns:w="{W}"><w:style w:styleId="x"><w:rPr><w:lang w:val="fr-FR"/></w:rPr></w:style></w:styles>"#
        );
        assert_eq!(default_lang(&none), None);
        let settings =
            format!(r#"<w:settings xmlns:w="{W}"><w:themeFontLang w:val="de-DE"/></w:settings>"#);
        assert_eq!(theme_font_lang(&settings).as_deref(), Some("de-DE"));
    }

    #[test]
    fn a_given_title_or_language_wins_and_each_is_cleaned() {
        let file = PdfInfo {
            title: Some("From the file".into()),
            lang: Some("en-AU".into()),
        };
        let given = PdfInfo {
            title: Some("Given".into()),
            lang: None,
        };
        let info = file.overridden_by(&given);
        assert_eq!(info.title.as_deref(), Some("Given"));
        assert_eq!(info.lang.as_deref(), Some("en-AU"));
        let odd = PdfInfo {
            title: Some(" a \t b\u{0007} ".into()),
            lang: Some("en) /JS (x".into()),
        };
        assert_eq!(odd.clean_title().as_deref(), Some("a b"));
        assert_eq!(odd.clean_lang(), None);
        assert_eq!(PdfInfo::default().clean_title(), None);
    }
}
