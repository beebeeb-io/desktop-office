//! The two error types of the API. Each is a code from a fixed list: the readers' own messages
//! (which can quote part names or text from the file) are never passed on.

/// Why a file could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The file is not a readable document of the kind asked for.
    Damaged,
    /// The file is over one of the limits ([`crate::Limits`]); the text names which one
    /// ([`crate::limits::UNPACKED`], [`crate::limits::ENTRIES`], [`crate::limits::PAGES`] or
    /// [`crate::limits::READER`]).
    TooLarge(&'static str),
    /// This kind of file is not shown by this build.
    Unsupported,
    /// The file is protected with a password (or encrypted in a way that needs one).
    Password,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Damaged => f.write_str("damaged"),
            Self::TooLarge(what) => write!(f, "too large: {what}"),
            Self::Unsupported => f.write_str("unsupported"),
            Self::Password => f.write_str("password"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why a page, slide or picture could not be rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// There is no page (slide, anchor) with that number.
    NoSuchPage,
    /// The scale is not a finite number above 0.
    BadScale,
    /// The result would be more than [`crate::Limits::max_render_side_px`] pixels on a side.
    TooLarge,
    /// The region is empty or does not lie inside the page at that scale.
    BadRegion,
    /// The output buffer is not exactly `w * h * 4` bytes.
    BufferSize,
    /// The renderer returned a raster of another size than asked for; nothing was written.
    Engine,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoSuchPage => "no such page",
            Self::BadScale => "bad scale",
            Self::TooLarge => "too large",
            Self::BadRegion => "bad region",
            Self::BufferSize => "buffer size",
            Self::Engine => "engine",
        })
    }
}

impl std::error::Error for RenderError {}

/// The `.docx` reader's error as a code. Every variant is listed, so a new one upstream is a
/// compile error here and gets read before a subtree bump lands.
pub(crate) fn from_docx(e: wordcraft_docx::DocxError) -> OpenError {
    use wordcraft_docx::DocxError as E;
    match e {
        E::PasswordRequired | E::WrongPassword | E::Encryption(_) => OpenError::Password,
        E::Limit(_) => OpenError::TooLarge(crate::limits::READER),
        E::Zip(_) | E::MissingPart(_) | E::Xml(_) | E::NotWord(_) | E::NotWordXml => OpenError::Damaged,
    }
}

/// The `.doc` reader's error as a code (every variant listed, as in [`from_docx`]).
pub(crate) fn from_docbin(e: wordcraft_docbin::DocbinError) -> OpenError {
    use wordcraft_docbin::DocbinError as E;
    match e {
        E::PasswordRequired | E::WrongPassword | E::Encryption(_) => OpenError::Password,
        E::Limit(_) => OpenError::TooLarge(crate::limits::READER),
        E::Container(_) | E::NotWord(_) | E::Malformed(_) => OpenError::Damaged,
    }
}
