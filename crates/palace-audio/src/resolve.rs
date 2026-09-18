//! Turning a script's sound name into bytes or a URL.
//!
//! This mirrors the reference client's `playSound`:
//!
//! 1. look the lower-cased name up in the bundled map;
//! 2. otherwise drop a trailing `.wav`, lower-case it and fetch
//!    `<mediaServer>/<name>.mp3`.
//!
//! The bundled table is empty — those MP3s are not ours to ship — so step 1 is
//! a real branch with no answers today. It is still the first branch, and the
//! order is what [`resolve_sound_with`] tests exercise.

/// The names the reference client ships as MP3s, and nothing else.
///
/// Empty on purpose: a build that has the assets may drop them in as
/// `("chime", include_bytes!("..."))` pairs and the first branch starts
/// answering. The map's keys are lower-case; lookups lower-case the name first.
pub const BUNDLED_SOUNDS: &[(&str, &[u8])] = &[];

/// Where a `SOUND name` will come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundSource<'a> {
    /// One of the bundled assets.
    Bundled(&'a [u8]),
    /// A media name to fetch, already normalised to `<name>.mp3`.
    Media(String),
}

/// The lower-cased name to fetch for a `SOUND`, or `None` if the name is unusable.
///
/// A trailing `.wav` is dropped, matching the reference's
/// `soundName.replace(/\.wav$/i, "")`; the result is lower-cased because the
/// reference lower-cases before joining the media server.
#[must_use]
pub fn sound_media_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".wav").unwrap_or(&lower);
    if stem.is_empty() {
        return None;
    }
    Some(format!("{stem}.mp3"))
}

/// The name to fetch for a `MIDIPLAY`/`MIDILOOP`.
///
/// A name that already carries an extension is used as written; one that does
/// not gets `.mid`, matching the reference's `/(.*)\.(.*)/` test.
#[must_use]
pub fn midi_media_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains('.') {
        Some(name.to_string())
    } else {
        Some(format!("{name}.mid"))
    }
}

/// Resolve against a caller-supplied table, so the lookup order is testable
/// without shipping assets.
#[must_use]
pub fn resolve_sound_with<'a>(
    table: &'a [(&'a str, &'a [u8])],
    name: &str,
) -> Option<SoundSource<'a>> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if let Some((_, bytes)) = table.iter().find(|(key, _)| *key == lower) {
        return Some(SoundSource::Bundled(bytes));
    }
    sound_media_name(name).map(SoundSource::Media)
}

/// Resolve against the shipped [`BUNDLED_SOUNDS`] table.
#[must_use]
pub fn resolve_sound(name: &str) -> Option<SoundSource<'static>> {
    resolve_sound_with(BUNDLED_SOUNDS, name)
}

/// The bundled bytes for `name`, if the shipped table has them.
#[must_use]
pub fn bundled_sound(name: &str) -> Option<&'static [u8]> {
    match resolve_sound(name) {
        Some(SoundSource::Bundled(bytes)) => Some(bytes),
        _ => None,
    }
}

/// The absolute URL a `SOUND` resolves to, or `None` with no base or no name.
#[must_use]
pub fn sound_url(base: &str, name: &str) -> Option<String> {
    if base.trim().is_empty() {
        return None;
    }
    let media = sound_media_name(name)?;
    Some(palace_asset::media_url(base, &media))
}

/// The absolute URL a `MIDIPLAY`/`MIDILOOP` resolves to.
#[must_use]
pub fn midi_url(base: &str, name: &str) -> Option<String> {
    if base.trim().is_empty() {
        return None;
    }
    let media = midi_media_name(name)?;
    Some(palace_asset::media_url(base, &media))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundled_name_wins_before_the_media_url() {
        static TABLE: &[(&str, &[u8])] = &[("chime", b"bundled")];
        assert_eq!(
            resolve_sound_with(TABLE, "Chime"),
            Some(SoundSource::Bundled(b"bundled")),
            "the map is consulted case-insensitively first"
        );
        assert_eq!(
            resolve_sound_with(TABLE, "chime.wav"),
            Some(SoundSource::Media("chime.mp3".to_string())),
            "the reference looks the name up before stripping .wav, so the suffix misses the map"
        );
    }

    #[test]
    fn an_unbundled_name_resolves_to_its_media_mp3() {
        assert_eq!(
            resolve_sound_with(&[], "garden"),
            Some(SoundSource::Media("garden.mp3".to_string()))
        );
    }

    #[test]
    fn a_wav_suffix_is_dropped_and_the_name_lowercased() {
        assert_eq!(
            resolve_sound_with(&[], "Garden.WAV"),
            Some(SoundSource::Media("garden.mp3".to_string()))
        );
    }

    #[test]
    fn an_empty_name_resolves_to_nothing() {
        assert_eq!(resolve_sound_with(&[], "   "), None);
        assert_eq!(resolve_sound_with(&[], ""), None);
        assert_eq!(sound_media_name(".wav"), None);
    }

    #[test]
    fn the_shipped_table_is_empty_but_the_lookup_still_works() {
        assert_eq!(BUNDLED_SOUNDS.len(), 0);
        assert_eq!(bundled_sound("chime"), None);
        assert_eq!(
            resolve_sound("chime"),
            Some(SoundSource::Media("chime.mp3".to_string()))
        );
    }

    #[test]
    fn a_sound_url_joins_the_media_base() {
        assert_eq!(
            sound_url("https://host/media", "garden"),
            Some("https://host/media/garden.mp3".to_string())
        );
        assert_eq!(
            sound_url("https://host/media/", "garden.wav"),
            Some("https://host/media/garden.mp3".to_string())
        );
        assert_eq!(sound_url("", "garden"), None);
    }

    #[test]
    fn midi_names_get_a_mid_extension_only_when_one_is_missing() {
        assert_eq!(midi_media_name("garden"), Some("garden.mid".to_string()));
        assert_eq!(
            midi_media_name("garden.mid"),
            Some("garden.mid".to_string())
        );
        assert_eq!(
            midi_media_name("garden.MID"),
            Some("garden.MID".to_string())
        );
        assert_eq!(midi_media_name("  "), None);
    }

    #[test]
    fn a_midi_url_joins_the_media_base() {
        assert_eq!(
            midi_url("https://host/media", "garden"),
            Some("https://host/media/garden.mid".to_string())
        );
        assert_eq!(midi_url("", "garden"), None);
    }
}
