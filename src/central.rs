use std::sync::mpsc::{self, Receiver, Sender};

use chrono::{DateTime, Local};
use regex::Regex;

use crate::{
    log_debug,
    mpv_commander::{mpv_load_url, mpv_seek, MpvIpcRequest, MpvIpcResponse},
    vrc_log_reader::{FoundSeek, UrlAndSeekResult, VrcLogReader, VrcLogWatcherEvent},
};

pub(crate) enum CentralCommand {
    MpvIpcEvent(MpvIpcResponse),
    VrcLogWatcherEvent(VrcLogWatcherEvent),
}

pub(crate) struct Central {
    central_rx: Receiver<CentralCommand>,
    pub(crate) central_tx: Sender<CentralCommand>,
    mpv_ipc_tx: Sender<MpvIpcRequest>,
    mid_loading_initial_state: Option<InitialState>,
}

struct InitialStateResult {
    initial_state: Option<InitialState>,
    lines_read_initially: u64,
}

struct InitialState {
    timestamp: DateTime<Local>,
    additional_offset: f64,
    url: String,
}

impl Central {
    pub(crate) fn new(mpv_ipc_tx: Sender<MpvIpcRequest>) -> Self {
        let (central_tx, central_rx) = mpsc::channel::<CentralCommand>();
        Self {
            central_rx,
            central_tx,
            mpv_ipc_tx,
            mid_loading_initial_state: None,
        }
    }

    pub(crate) fn do_initial(&mut self, player_name_regex: &Option<Regex>) -> u64 {
        let initial_state_result = read_initial_state_from_log(player_name_regex);

        if let Some(initial_state) = initial_state_result.initial_state {
            log_debug!("Initial state found.");
            log_debug!("Timestamp: {}", initial_state.timestamp);
            log_debug!("Additional offset: {}", initial_state.additional_offset);
            log_debug!("Given that right now is {}", chrono::Local::now());
            log_debug!(
                "We should seek to {}",
                calculate_seek_from_initial_state(&initial_state)
            );

            mpv_load_url(self.mpv_ipc_tx.clone(), &initial_state.url);
            self.mid_loading_initial_state = Some(initial_state);
        }

        initial_state_result.lines_read_initially
    }

    pub(crate) fn run_central_dispatch(mut self) {
        for response in self.central_rx {
            match response {
                CentralCommand::MpvIpcEvent(MpvIpcResponse::PlaybackRestart) => {
                    if let Some(initial_state) = self.mid_loading_initial_state {
                        self.mid_loading_initial_state = None;
                        // we were waiting to load. now we can seek.
                        let target_timestamp = calculate_seek_from_initial_state(&initial_state);
                        mpv_seek(self.mpv_ipc_tx.clone(), target_timestamp);
                    }
                }
                CentralCommand::MpvIpcEvent(MpvIpcResponse::FileLoaded) => {}
                CentralCommand::VrcLogWatcherEvent(VrcLogWatcherEvent::FoundUrl(found_url)) => {
                    mpv_load_url(self.mpv_ipc_tx.clone(), &found_url.url);
                }
                CentralCommand::VrcLogWatcherEvent(VrcLogWatcherEvent::FoundSeek(found_seek)) => {
                    let new_timestamp = timestamp_from_seek_line(&found_seek);
                    mpv_seek(self.mpv_ipc_tx.clone(), new_timestamp);
                }
            }
        }
    }
}

fn timestamp_from_seek_line(found_seek: &FoundSeek) -> f64 {
    // how long has it been since this timestamp?
    let now = chrono::Local::now();
    let duration = now.signed_duration_since(found_seek.timestamp);
    // add this duration to the seek offset, which has also been returned from the log file
    let new_seek_offset = found_seek.seek_offset + duration.num_milliseconds() as f64 / 1000.0;

    new_seek_offset
}

// Do this as late as possible.
fn calculate_seek_from_initial_state(initial_state: &InitialState) -> f64 {
    // how long has it been since this timestamp?
    let now = chrono::Local::now();
    let duration = now.signed_duration_since(initial_state.timestamp);
    // add this duration to the seek offset, which may have also been returned from the log file
    let new_seek_offset =
        initial_state.additional_offset + duration.num_milliseconds() as f64 / 1000.0;

    new_seek_offset
}

fn read_initial_state_from_log(player_name_regex: &Option<Regex>) -> InitialStateResult {
    let mut vlr = VrcLogReader::from_latest(player_name_regex);
    let url_and_seek = vlr.get_latest_url_and_seek();
    match url_and_seek {
        UrlAndSeekResult::Nothing(lines_read_initially) => {
            log_debug!("No URL found in the log file so far. We'll wait for some.");
            InitialStateResult {
                initial_state: None,
                lines_read_initially,
            }
        }
        UrlAndSeekResult::Url(found_url, lines_read_initially) => {
            log_debug!("URL found: {}", found_url.url);

            InitialStateResult {
                initial_state: Some(InitialState {
                    timestamp: found_url.timestamp,
                    additional_offset: 0.0,
                    url: found_url.url,
                }),
                lines_read_initially,
            }
        }
        UrlAndSeekResult::UrlAndSeek(found_url, found_seek, lines_read_initially) => {
            InitialStateResult {
                initial_state: Some(InitialState {
                    timestamp: found_seek.timestamp,
                    additional_offset: found_seek.seek_offset,
                    url: found_url.url,
                }),
                lines_read_initially,
            }
        }
    }
}
