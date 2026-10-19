use std::{
    io::SeekFrom,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Poll, Waker},
    time::{Duration, Instant},
};

use anyhow::Context;
use dashmap::DashMap;

use librqbit_core::lengths::{CurrentPiece, Lengths, ValidPieceIndex};
use tokio::io::{AsyncRead, AsyncSeek};
use tracing::{debug, trace};

use crate::{ManagedTorrent, file_info::FileInfo, storage::TorrentStorage};

use super::{ManagedTorrentHandle, TorrentMetadata};

type StreamId = usize;

// MV-PATCH: the upstream fixed 32 MiB lookahead with shuffled interleaving
// across streams is replaced by:
//  * a per-stream "near" window sized from the file length (~1 minute of
//    video for typical bitrates), overridable via `FileStream::set_priority`;
//  * streams ordered by recency (newest open / seek first), so a seek
//    immediately outranks the stale window of the previous position;
//  * "high" windows (container index warm-up: first/last pieces) before all
//    near windows;
//  * after every near window, each stream continues sequentially to the end
//    of its file (Stremio/peerflix style) instead of falling back to
//    downloading from the start of the file.
const MIN_READAHEAD: u64 = 16 * 1024 * 1024;
const MAX_READAHEAD: u64 = 128 * 1024 * 1024;
/// Bytes after the playhead whose pieces count as "critical" (deadline
/// pieces that may be stolen from slow peers).
const CRITICAL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CRITICAL_PIECES_PER_STREAM: u32 = 4;
/// Only the most recent streams get critical treatment.
const MAX_CRITICAL_STREAMS: usize = 1;
/// Pieces of a critical stream's near window that are fetched block-striped
/// across peers (whole swarm works on the earliest pieces first).
const MAX_STRIPED_PIECES: u32 = 64;
const MAX_STRIPED_BYTES: u64 = 32 * 1024 * 1024;

/// Default near window: ~1/120 of the file (about a minute for a 2 h movie),
/// clamped to [16 MiB, 128 MiB].
pub(crate) fn default_readahead(file_len: u64) -> u64 {
    (file_len / 120).clamp(MIN_READAHEAD, MAX_READAHEAD)
}

struct StreamState {
    file_id: usize,
    file_len: u64,
    file_abs_offset: u64,
    position: u64,
    waker: Option<Waker>,
    /// Recency: bumped on open and on every seek. Higher = more important.
    recency: u64,
    /// Near window in bytes after `position`.
    window: u64,
    /// Continue sequentially to the end of the file after the near window.
    to_eof: bool,
    /// Warm-up window that outranks every near window.
    high: bool,
    /// When `recency` was last bumped (open / seek).
    recency_at: Instant,
}

impl StreamState {
    fn current_piece(&self, lengths: &Lengths) -> Option<CurrentPiece> {
        lengths.compute_current_piece(self.position, self.file_abs_offset)
    }

    /// Piece ids covering `[from, from + len)` of this file (clamped to it).
    fn piece_range(&self, lengths: &Lengths, from: u64, len: u64) -> std::ops::Range<u32> {
        let file_end = self.file_abs_offset + self.file_len;
        let start = (self.file_abs_offset + from).min(file_end);
        let end = start.saturating_add(len).min(file_end);
        let dpl = lengths.default_piece_length() as u64;
        let start_id: u32 = (start / dpl).try_into().unwrap();
        let end_id: u32 = end.div_ceil(dpl).try_into().unwrap();
        start_id..end_id.max(start_id)
    }

    fn near_range(&self, lengths: &Lengths) -> std::ops::Range<u32> {
        self.piece_range(lengths, self.position, self.window.max(1))
    }

    fn eof_range(&self, lengths: &Lengths) -> std::ops::Range<u32> {
        if !self.to_eof {
            return 0..0;
        }
        self.piece_range(lengths, self.position, u64::MAX)
    }

    fn critical_range(&self, lengths: &Lengths) -> std::ops::Range<u32> {
        let r = self.piece_range(lengths, self.position, CRITICAL_BYTES.min(self.window.max(1)));
        r.start..r.end.min(r.start + MAX_CRITICAL_PIECES_PER_STREAM)
    }
}

#[derive(Default)]
pub(crate) struct TorrentStreams {
    next_stream_id: AtomicUsize,
    // MV-PATCH: recency counter for priority ordering.
    recency: AtomicU64,
    streams: DashMap<StreamId, StreamState>,
}

impl TorrentStreams {
    fn next_id(&self) -> usize {
        self.next_stream_id.fetch_add(1, Ordering::Relaxed)
    }

    fn next_recency(&self) -> u64 {
        self.recency.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn register_waker(&self, stream_id: StreamId, waker: Waker) {
        if let Some(mut s) = self.streams.get_mut(&stream_id) {
            let vm = s.value_mut();
            vm.waker = Some(waker);
        }
    }

    /// MV-PATCH: priority order = high windows (newest first), then near
    /// windows (newest first), then each stream's continuation to EOF
    /// (newest first). Duplicates are harmless (the picker skips pieces it
    /// has or that are in flight). Ranges are snapshotted so no DashMap shard
    /// lock is held while the caller iterates.
    pub(crate) fn iter_next_pieces<'a>(
        &'a self,
        lengths: &'a Lengths,
    ) -> impl Iterator<Item = ValidPieceIndex> + 'a {
        // (high, recency, near, eof)
        let mut all: Vec<(bool, u64, std::ops::Range<u32>, std::ops::Range<u32>)> = self
            .streams
            .iter()
            .map(|s| (s.high, s.recency, s.near_range(lengths), s.eof_range(lengths)))
            .collect();
        all.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let near: Vec<std::ops::Range<u32>> = all.iter().map(|r| r.2.clone()).collect();
        all.sort_by(|a, b| b.1.cmp(&a.1));
        let eof: Vec<std::ops::Range<u32>> = all.into_iter().map(|r| r.3).collect();
        near.into_iter()
            .chain(eof)
            .flatten()
            .filter_map(|i| lengths.validate_piece_index(i))
    }

    /// MV-PATCH: what a seek must not cancel: the newest stream's near window
    /// plus all high (warm-up) windows. Also returns the newest stream's
    /// current piece.
    pub(crate) fn preempt_keep(
        &self,
        lengths: &Lengths,
    ) -> Option<(Vec<std::ops::Range<u32>>, u32)> {
        let mut newest: Option<(u64, std::ops::Range<u32>)> = None;
        let mut keep = Vec::new();
        for s in self.streams.iter() {
            let near = s.near_range(lengths);
            if s.high {
                keep.push(near.clone());
            }
            if newest.as_ref().is_none_or(|(r, _)| s.recency > *r) {
                newest = Some((s.recency, near));
            }
        }
        let (_, near) = newest?;
        let first = near.start;
        keep.push(near);
        Some((keep, first))
    }

    /// MV-PATCH: for the newest stream(s), in priority order, the pieces
    /// that are fetched block-striped across fast peers (start of the near
    /// window), flagged `urgent` for the pieces right at the playhead (those
    /// also get endgame-style duplicate requests). Also returns how long ago
    /// the newest stream was opened / seeked.
    pub(crate) fn critical_pieces_and_age(
        &self,
        lengths: &Lengths,
    ) -> (Vec<(ValidPieceIndex, bool)>, Duration) {
        let age = self
            .streams
            .iter()
            .max_by_key(|s| s.recency)
            .map(|s| s.recency_at.elapsed())
            .unwrap_or(Duration::MAX);
        (self.critical_pieces_inner(lengths), age)
    }

    fn critical_pieces_inner(&self, lengths: &Lengths) -> Vec<(ValidPieceIndex, bool)> {
        let mut all: Vec<(u64, std::ops::Range<u32>, std::ops::Range<u32>)> = self
            .streams
            .iter()
            .map(|s| {
                let striped = s.piece_range(
                    lengths,
                    s.position,
                    s.window.clamp(1, MAX_STRIPED_BYTES),
                );
                let striped = striped.start..striped.end.min(striped.start + MAX_STRIPED_PIECES);
                (s.recency, striped, s.critical_range(lengths))
            })
            .collect();
        all.sort_by(|a, b| b.0.cmp(&a.0));
        let all: Vec<_> = all.into_iter().take(MAX_CRITICAL_STREAMS).collect();
        // Urgent pieces of every critical stream first, then the rest.
        let urgent = all.iter().flat_map(|(_, _, u)| u.clone()).map(|i| (i, true));
        let rest = all
            .iter()
            .flat_map(|(_, s, u)| s.clone().filter(move |i| !u.contains(i)))
            .map(|i| (i, false));
        urgent
            .chain(rest)
            .filter_map(|(i, u)| Some((lengths.validate_piece_index(i)?, u)))
            .collect()
    }

    pub(crate) fn wake_streams_on_piece_completed(
        &self,
        piece_id: ValidPieceIndex,
        lengths: &Lengths,
    ) {
        for mut w in self.streams.iter_mut() {
            if w.value().current_piece(lengths).map(|p| p.id) == Some(piece_id)
                && let Some(waker) = w.value_mut().waker.take()
            {
                debug!(
                    stream_id = *w.key(),
                    piece_id = piece_id.get(),
                    "waking stream"
                );
                waker.wake();
            }
        }
    }

    fn drop_stream(&self, stream_id: StreamId) -> Option<StreamState> {
        debug!(stream_id, "dropping stream");
        self.streams.remove(&stream_id).map(|s| s.1)
    }

    pub(crate) fn streamed_file_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.streams.iter().map(|s| s.value().file_id)
    }
}

pub struct FileStream {
    torrent: ManagedTorrentHandle,
    metadata: Arc<TorrentMetadata>,
    streams: Arc<TorrentStreams>,
    stream_id: usize,
    file_id: usize,
    position: u64,

    // file params
    file_len: u64,
    file_torrent_abs_offset: u64,
    // MV-PATCH: upstream held a blocking-spawner permit for the whole life of
    // the stream (8 by default, shared with add_torrent), so a few stale HTTP
    // connections could stall new streams and new torrents. Removed.
}

macro_rules! map_io_err {
    ($e:expr) => {
        $e.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    };
}

macro_rules! poll_try_io {
    ($e:expr) => {{
        let e = map_io_err!($e);
        match e {
            Ok(r) => r,
            Err(e) => {
                debug!("stream error {e:#}");
                return Poll::Ready(Err(e));
            }
        }
    }};
}

impl AsyncRead for FileStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        tbuf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // if the file is over, return 0
        if self.position == self.file_len {
            debug!(
                stream_id = self.stream_id,
                file_id = self.file_id,
                "stream completed, EOF"
            );
            return Poll::Ready(Ok(()));
        }

        let current = poll_try_io!(
            self.metadata
                .lengths()
                .compute_current_piece(self.position, self.file_torrent_abs_offset)
                .context("invalid position")
        );

        // if the piece is not there, register to wake when it is
        // check if we have the piece for real
        let have = poll_try_io!(self.torrent.with_chunk_tracker(|ct| {
            let have = ct.get_have_pieces().as_slice()[current.id.get() as usize];
            if !have {
                self.streams
                    .register_waker(self.stream_id, cx.waker().clone());
            }
            have
        }));
        if !have {
            debug!(stream_id = self.stream_id, file_id = self.file_id, piece_id = %current.id, "poll pending, not have");
            return Poll::Pending;
        }

        // actually stream the piece
        let buf = tbuf.initialize_unfilled();
        let file_remaining = self.file_len - self.position;
        let bytes_to_read: usize = poll_try_io!(
            (buf.len() as u64)
                .min(current.piece_remaining as u64)
                .min(file_remaining)
                .try_into()
        );

        let buf = &mut buf[..bytes_to_read];

        let start = Instant::now();
        poll_try_io!(poll_try_io!(self.torrent.shared.spawner.block_in_place(
            || {
                self.torrent.with_storage_and_file(
                    self.file_id,
                    |files, _fi| {
                        files.pread_exact(self.file_id, self.position, buf)?;
                        Ok::<_, anyhow::Error>(())
                    },
                    &self.metadata,
                )
            }
        )));

        trace!(
            buflen = buf.len(),
            stream_id = self.stream_id,
            file_id = self.file_id,
            read_time = ?start.elapsed(),
            "will write bytes"
        );

        self.as_mut().advance(bytes_to_read as u64);
        tbuf.advance(bytes_to_read);

        Poll::Ready(Ok(()))
    }
}

impl AsyncSeek for FileStream {
    fn start_seek(
        mut self: std::pin::Pin<&mut Self>,
        position: std::io::SeekFrom,
    ) -> std::io::Result<()> {
        let end_i64 = map_io_err!(TryInto::<i64>::try_into(self.file_len))?;
        let new_pos: i64 = match position {
            SeekFrom::Start(s) => map_io_err!(s.try_into())?,
            SeekFrom::End(e) => map_io_err!(TryInto::<i64>::try_into(self.file_len))? + e,
            SeekFrom::Current(o) => map_io_err!(TryInto::<i64>::try_into(self.position))? + o,
        };

        if new_pos < 0 || new_pos > end_i64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                anyhow::anyhow!("invalid seek"),
            ));
        }

        self.as_mut().set_position(map_io_err!(new_pos.try_into())?);
        // MV-PATCH: a seek makes this stream the most important one.
        let recency = self.streams.next_recency();
        if let Some(mut st) = self.streams.streams.get_mut(&self.stream_id) {
            st.recency = recency;
            st.recency_at = Instant::now();
        }
        // ... and, if the data there isn't already coming, cancels the old
        // window so every peer's queue is free for the new position.
        if let Some(live) = self.torrent.live() {
            live.preempt_for_seek();
        }
        debug!(stream_id = self.stream_id, position = self.position, "seek");
        Ok(())
    }

    fn poll_complete(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Ok(self.position))
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        self.streams.drop_stream(self.stream_id);
    }
}

impl ManagedTorrent {
    fn with_storage_and_file<F, R>(
        &self,
        file_id: usize,
        f: F,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<R>
    where
        F: FnOnce(&dyn TorrentStorage, &FileInfo) -> R,
    {
        self.with_state(|s| {
            let files = match s {
                crate::ManagedTorrentState::Paused(p) => &*p.files,
                crate::ManagedTorrentState::Live(l) => &*l.files,
                s => anyhow::bail!("with_storage_and_file: invalid state: {}", s.name()),
            };
            let fi = metadata.file_infos.get(file_id).context("invalid file")?;
            Ok(f(files, fi))
        })
    }

    fn streams(&self) -> anyhow::Result<Arc<TorrentStreams>> {
        self.with_state(|s| match s {
            crate::ManagedTorrentState::Paused(p) => Ok(p.streams.clone()),
            crate::ManagedTorrentState::Live(l) => Ok(l.streams.clone()),
            s => anyhow::bail!("streams: invalid state {}", s.name()),
        })
    }

    fn maybe_reconnect_needed_peers_for_file(&self, file_id: usize) -> bool {
        // If we have the full file, don't bother.
        if self.is_file_finished(file_id) {
            return false;
        }
        self.with_state(|state| {
            if let crate::ManagedTorrentState::Live(l) = &state {
                l.reconnect_all_not_needed_peers();
            }
        });
        true
    }

    fn is_file_finished(&self, file_id: usize) -> bool {
        let metadata = self.metadata.load();
        let metadata = match metadata.as_ref() {
            Some(r) => r,
            None => return false,
        };
        // TODO: would be nice to remove locking
        self.with_chunk_tracker(|ct| ct.is_file_finished(&metadata.file_infos[file_id]))
            .unwrap_or(false)
    }

    pub async fn stream(self: Arc<Self>, file_id: usize) -> anyhow::Result<FileStream> {
        let metadata = self
            .metadata
            .load_full()
            .context("torrent metadata is not resolved")?;
        let (fd_len, fd_offset) = self.with_storage_and_file(
            file_id,
            |_fd, fi| (fi.len, fi.offset_in_torrent),
            &metadata,
        )?;
        let streams = self.streams()?;
        let s = FileStream {
            stream_id: streams.next_id(),
            streams: streams.clone(),
            file_id,
            position: 0,

            file_len: fd_len,
            file_torrent_abs_offset: fd_offset,
            torrent: self,
            metadata,
        };
        s.torrent.maybe_reconnect_needed_peers_for_file(file_id);
        streams.streams.insert(
            s.stream_id,
            StreamState {
                file_id,
                position: 0,
                waker: None,
                file_len: fd_len,
                file_abs_offset: fd_offset,
                recency: streams.next_recency(),
                recency_at: Instant::now(),
                window: default_readahead(fd_len),
                to_eof: true,
                high: false,
            },
        );

        debug!(stream_id = s.stream_id, file_id, "started stream");

        Ok(s)
    }
}

impl FileStream {
    pub fn position(&self) -> u64 {
        self.position
    }

    fn advance(&mut self, diff: u64) {
        self.set_position(self.position + diff)
    }

    fn set_position(&mut self, new_pos: u64) {
        self.position = new_pos;
        self.streams
            .streams
            .get_mut(&self.stream_id)
            .unwrap()
            .value_mut()
            .position = new_pos;
    }

    pub fn len(&self) -> u64 {
        self.file_len
    }

    /// MV-PATCH: tune how this stream drives piece selection.
    /// `window`: bytes after the read position fetched with priority (the
    /// readahead). `to_eof`: keep fetching sequentially to the end of the file
    /// after the window. `high`: warm-up window that outranks all others
    /// (used for container index prefetch: first/last pieces).
    pub fn set_priority(&mut self, window: u64, to_eof: bool, high: bool) {
        if let Some(mut st) = self.streams.streams.get_mut(&self.stream_id) {
            st.window = window;
            st.to_eof = to_eof;
            st.high = high;
        }
    }

    /// MV-PATCH: whether every piece overlapping `[position, position + len)`
    /// of this file is downloaded and verified.
    pub fn is_range_available(&self, position: u64, len: u64) -> bool {
        let lengths = self.metadata.lengths();
        let file_end = self.file_torrent_abs_offset + self.file_len;
        let start = (self.file_torrent_abs_offset + position).min(file_end);
        let end = start.saturating_add(len).min(file_end);
        if start >= end {
            return true;
        }
        let dpl = lengths.default_piece_length() as u64;
        let (a, b) = ((start / dpl) as usize, end.div_ceil(dpl) as usize);
        self.torrent
            .with_chunk_tracker(|ct| {
                let have = ct.get_have_pieces();
                let have = have.as_slice();
                (a..b).all(|i| have.get(i).map(|v| *v).unwrap_or(false))
            })
            .unwrap_or(false)
    }
}
