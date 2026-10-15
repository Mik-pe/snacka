// SPDX-License-Identifier: MIT
//! Borrowed [Opus padding-extension framing][spec] without allocation.
//!
//! This parses structural IDs 0, 1 and 2 and exposes the other IDs as opaque
//! payloads. It does not implement DRED, QEXT or any other audio extension.
//! Invalid extensions do not invalidate an otherwise valid base Opus packet.
//!
//! [spec]: https://www.ietf.org/archive/id/draft-ietf-mlcodec-opus-extension-06.html

use super::packet::MAX_FRAMES;
use std::fmt;
use std::iter::FusedIterator;

/// Invalid optional extension framing, independent of base packet validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramingError {
    /// The supplied underlying frame count is outside 1 through 48.
    InvalidFrameCount,
    /// A separator associates subsequent extensions with a nonexistent frame.
    InvalidFrameIndex,
    /// An explicit extension length has no terminating byte.
    TruncatedLength,
    /// A payload, including reserved repeated short payloads, exceeds padding.
    TruncatedPayload,
}

impl fmt::Display for FramingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidFrameCount => "invalid Opus extension frame count",
            Self::InvalidFrameIndex => "Opus extension frame index exceeds packet",
            Self::TruncatedLength => "truncated Opus extension length",
            Self::TruncatedPayload => "Opus extension payload exceeds padding",
        })
    }
}

impl std::error::Error for FramingError {}

/// An opaque extension instance associated with one compressed Opus frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extension<'a> {
    /// Underlying frame index, starting at zero.
    pub frame_index: usize,
    /// Extension ID, from 3 through 127. Structural IDs are not yielded.
    pub id: u8,
    /// Extension data borrowed from the original padding, excluding framing.
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
struct Template {
    offset: usize,
    id: u8,
    flag: bool,
}

#[derive(Debug, Clone, Copy)]
struct Repeat {
    start: usize,
    end: usize,
    cursor: usize,
    frame_index: usize,
    last_long: Option<usize>,
    trailing_short_bytes: usize,
    flag: bool,
}

/// Allocation-free iterator over optional Opus padding extensions.
///
/// Valid instances preceding malformed framing are yielded, followed by exactly
/// one error; the iterator then ends. Consume it completely (or use
/// [`Self::validate`]) to check all extension framing. Base audio can still be
/// decoded after an extension error. Unknown IDs can simply be skipped.
///
/// Repeated instances can arrive before later instances for an earlier frame.
/// Order within each frame is preserved. No extension payloads are copied and
/// no fixed limit is imposed on their count. Repeat expansion and template scans
/// are bounded by the underlying frame count (at most 48) times padding size.
/// An individual `next()` call can scan a complete repeat group, including groups
/// containing only structural padding. Applications with a processing budget
/// should bound the packet/padding size before iteration; limiting yielded items
/// alone does not establish a constant work bound per call.
#[derive(Debug, Clone)]
pub struct Extensions<'a> {
    padding: &'a [u8],
    frame_count: usize,
    position: usize,
    frame_index: usize,
    group_start: usize,
    repeat: Option<Repeat>,
    finished: bool,
}

impl<'a> Extensions<'a> {
    /// Parse a raw padding region for an underlying packet of 1 through 48 frames.
    ///
    /// Use [`super::Packet::extensions`] when packet framing is available; it
    /// supplies the validated frame count and excludes padding length headers.
    pub fn new(padding: &'a [u8], frame_count: usize) -> Result<Self, FramingError> {
        if !(1..=MAX_FRAMES).contains(&frame_count) {
            return Err(FramingError::InvalidFrameCount);
        }
        Ok(Self::from_packet(padding, frame_count))
    }

    pub(crate) fn from_packet(padding: &'a [u8], frame_count: usize) -> Self {
        Self {
            padding,
            frame_count,
            position: 0,
            frame_index: 0,
            group_start: 0,
            repeat: None,
            finished: false,
        }
    }

    /// Consume and validate all remaining extension framing without allocation.
    pub fn validate(mut self) -> Result<(), FramingError> {
        self.try_for_each(|extension| extension.map(|_| ()))
    }

    fn next_inner(&mut self) -> Result<Option<Extension<'a>>, FramingError> {
        loop {
            if let Some(mut repeat) = self.repeat {
                if let Some(template) = next_template(self.padding, &mut repeat.cursor, repeat.end)?
                {
                    let final_long = repeat.frame_index == self.frame_count - 1
                        && repeat.last_long == Some(template.offset);
                    let flag = if template.id >= 32 {
                        !final_long || repeat.flag
                    } else {
                        template.flag
                    };
                    let reserve = if final_long && !flag {
                        repeat.trailing_short_bytes
                    } else {
                        0
                    };
                    let payload = read_payload(
                        self.padding,
                        &mut self.position,
                        self.padding.len(),
                        template.id,
                        flag,
                        reserve,
                    )?;
                    self.repeat = Some(repeat);
                    return Ok(Some(Extension {
                        frame_index: repeat.frame_index,
                        id: template.id,
                        payload,
                    }));
                }
                repeat.frame_index += 1;
                if repeat.frame_index < self.frame_count {
                    repeat.cursor = repeat.start;
                    self.repeat = Some(repeat);
                } else {
                    self.repeat = None;
                    self.group_start = self.position;
                    if !repeat.flag && repeat.last_long.is_none() {
                        self.frame_index += 1;
                    }
                }
                continue;
            }

            let Some(&byte) = self.padding.get(self.position) else {
                return Ok(None);
            };
            let offset = self.position;
            self.position += 1;
            let id = byte >> 1;
            let flag = byte & 1 != 0;
            match id {
                0 if !flag => return Ok(None),
                0 => {}
                1 => {
                    let increment = if flag {
                        let value = *self
                            .padding
                            .get(self.position)
                            .ok_or(FramingError::TruncatedPayload)?;
                        self.position += 1;
                        usize::from(value)
                    } else {
                        1
                    };
                    self.frame_index += increment;
                    if self.frame_index >= self.frame_count {
                        return Err(FramingError::InvalidFrameIndex);
                    }
                    if increment != 0 {
                        self.group_start = self.position;
                    }
                }
                2 => {
                    if self.frame_index == self.frame_count - 1 {
                        self.group_start = self.position;
                        if !flag {
                            return Ok(None);
                        }
                        continue;
                    }
                    let mut cursor = self.group_start;
                    let mut last_long = None;
                    let mut trailing_short_bytes = 0;
                    while let Some(template) = next_template(self.padding, &mut cursor, offset)? {
                        if template.id >= 32 {
                            last_long = Some(template.offset);
                            trailing_short_bytes = 0;
                        } else {
                            trailing_short_bytes += usize::from(template.flag);
                        }
                    }
                    self.repeat = Some(Repeat {
                        start: self.group_start,
                        end: offset,
                        cursor: self.group_start,
                        frame_index: self.frame_index + 1,
                        last_long,
                        trailing_short_bytes,
                        flag,
                    });
                }
                _ => {
                    let payload = read_payload(
                        self.padding,
                        &mut self.position,
                        self.padding.len(),
                        id,
                        flag,
                        0,
                    )?;
                    return Ok(Some(Extension {
                        frame_index: self.frame_index,
                        id,
                        payload,
                    }));
                }
            }
        }
    }
}

impl<'a> Iterator for Extensions<'a> {
    type Item = Result<Extension<'a>, FramingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.next_inner() {
            Ok(Some(extension)) => Some(Ok(extension)),
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

impl FusedIterator for Extensions<'_> {}

// A group contains already validated ordinary extensions, single-byte padding,
// and zero-increment separators. Prior RTE payloads are outside the group.
fn next_template(
    padding: &[u8],
    cursor: &mut usize,
    end: usize,
) -> Result<Option<Template>, FramingError> {
    while *cursor < end {
        let offset = *cursor;
        let byte = padding[*cursor];
        *cursor += 1;
        let id = byte >> 1;
        let flag = byte & 1 != 0;
        if id == 0 {
            continue;
        }
        if id == 1 {
            // Only L=1 separators with a zero increment can be in this group.
            *cursor += 1;
            continue;
        }
        read_payload(padding, cursor, end, id, flag, 0)?;
        return Ok(Some(Template { offset, id, flag }));
    }
    Ok(None)
}

fn read_payload<'a>(
    padding: &'a [u8],
    cursor: &mut usize,
    end: usize,
    id: u8,
    flag: bool,
    reserve: usize,
) -> Result<&'a [u8], FramingError> {
    let length = if id < 32 {
        usize::from(flag)
    } else if flag {
        let mut length = 0usize;
        loop {
            let byte = *padding
                .get(*cursor)
                .filter(|_| *cursor < end)
                .ok_or(FramingError::TruncatedLength)?;
            *cursor += 1;
            length = length
                .checked_add(usize::from(byte))
                .ok_or(FramingError::TruncatedPayload)?;
            if byte != 255 {
                break length;
            }
        }
    } else {
        (end - *cursor)
            .checked_sub(reserve)
            .ok_or(FramingError::TruncatedPayload)?
    };
    let payload_end = cursor
        .checked_add(length)
        .filter(|&position| position <= end)
        .ok_or(FramingError::TruncatedPayload)?;
    let payload = &padding[*cursor..payload_end];
    *cursor = payload_end;
    Ok(payload)
}
