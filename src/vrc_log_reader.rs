use lazy_regex::*;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Result},
    path::{Path, PathBuf},
    sync::mpsc,
};

use chrono::{DateTime, Local, TimeZone};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{log_debug, log_error};

fn try_match_seek_line(line: &String) -> Option<FoundSeek> {
    if let Some(captures) = &SEEK_REGEX.captures(&line) {
        let timestamp = captures.get(1).unwrap().as_str();
        let seek_offset = captures.get(4).unwrap().as_str();

        let timestamp = parse_timestamp(timestamp);
        // also, parse the seek offset as a floating point
        let seek_offset = seek_offset
            .parse::<f64>()
            .expect("Failed to parse seek offset as f64");
        return Some(FoundSeek {
            timestamp,
            seek_offset,
        });
    }

    None
}

fn parse_timestamp(timestamp: &str) -> DateTime<Local> {
    // timestamp is of the form:
    // 2024.04.22 17:55:53
    // parse it as local time:
    let timestamp = chrono::naive::NaiveDateTime::parse_from_str(&timestamp, "%Y.%m.%d %H:%M:%S")
        .expect("Failed to parse timestamp");
    let timestamp = chrono::Local
        .from_local_datetime(&timestamp)
        .earliest()
        .expect("Failed to convert timestamp to local time");
    timestamp
}

fn try_match_url_line(line: &String, line_number: u64) -> Option<FoundUrl> {
    if let Some(captures) = &URL_REGEX.captures(&line) {
        let timestamp = captures.get(1).unwrap().as_str();
        let url = captures.get(2).unwrap().as_str();
        let timestamp = parse_timestamp(timestamp);
        return Some(FoundUrl {
            timestamp,
            url: url.to_string(),
            found_url_on_line: line_number,
        });
    }

    None
}

fn get_latest_vrc_log_file() -> Option<PathBuf> {
    let log_dir = get_vrc_log_file_dir();
    let mut latest_log = None;

    // read dir
    if let Ok(entries) = fs::read_dir(&log_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Some(file_name) = entry.file_name().to_str() {
                    // for all log files,
                    if file_name.starts_with("output_log_") && file_name.ends_with(".txt") {
                        let file_path = entry.path();
                        if latest_log
                            .as_ref()
                            .map_or(true, |log| file_path > Path::new(log))
                        {
                            latest_log = Some(file_path);
                        }
                    }
                }
            }
        }
    }

    latest_log
}

fn get_vrc_log_file_dir() -> String {
    let log_dir = format!("{}/.steam/steam/steamapps/compatdata/438100/pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat", std::env::var("HOME").unwrap_or_default());
    log_dir
}

/*
We have several examples of log lines to choose from.

"2024.04.14 23:28:20 Log        -  [AT INFO        TVManager (Theatre 1)] [AVPro1080p_Theatre1] loading URL:
https://example.net/Media/Movies/spykids3d.mp4"
Initially, I had this one, but obviously this doesn't work if I'm not in the theatre.

"2024.04.14 21:25:36 Log        -  [AVProVideo] Opening http://example.com/mystream/stream.m3u8 (offset 0)
with API MediaFoundation"
AVProVideo might be a good option, but it likely doesn't capture usage of the Unity player, which iirc also needs
fixing.

"2024.04.14 21:25:36 Log        -  [Video Playback] URL 'http://example.com/mystream/index.m3u8' resolved to
'http://example.com/mystream/stream.m3u8'"
This one is more general, and might work everywhere, but it is 2 seconds delayed. If I don't need the resolution,
I'd prefer the earlier the better.

"2024.04.14 21:25:34 Log        -  [Video Playback] Attempting to resolve URL
'http://example.com/mystream/index.m3u8'"
I'll go with this one for now, as it's the earliest and the easiest.
*/
pub(crate) static URL_REGEX: Lazy<Regex> = lazy_regex!(
    r"^([0-9.: ]+) Log +- +\[Video Playback\] Attempting to resolve URL '(https?://\S+)'"
);

// this is specifically for ProTV.
// 2024.04.22 17:55:53 Log        -  [AT INFO    	TVManager (Theatre 1 TVManager)] Sync enforcement. Updating to 116.47
// 2024.05.09 19:11:19 Log        -  [AT DEBUG 	TVManager (Theatre 1 TVManager)] Paused drift threshold exceeded. Updating to 64.8041
pub(crate) static SEEK_REGEX: Lazy<Regex> = lazy_regex!(
    r"^([0-9.: ]+) Log +- +\[AT (INFO|DEBUG)[ \t]+TVManager \(.*\)\] (Sync enforcement|Paused drift threshold exceeded). Updating to ([0-9.]+)$"
);

fn tail_file<FCallback>(
    path: &PathBuf,
    start_after_line: u64,
    mut callback: FCallback,
) -> notify::Result<()>
where
    FCallback: FnMut(String, u64),
{
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&path, RecursiveMode::NonRecursive)?;

    let f = File::open(path)?;
    // skip ahead initially
    skip_n_lines(&f, start_after_line)?;
    // line numbers are 1-based. if I skip 3 lines, I am now at line 4.

    // view the file as lines, and keep track of the line number
    let lines = BufReader::new(&f).lines().enumerate();

    // read the rest of the file as it exists, calling the callback for each line
    for (i, line) in lines {
        let current_line_num = (i as u64) + start_after_line;
        let line = line.unwrap();
        callback(line, current_line_num);
    }

    // now, we'll keep watching the file for changes
    for res in rx {
        match res {
            Ok(_) => {
                // log_debug!("File changed with event: {:?}", event);

                // create a new BufReader for the file, because we're in the watcher loop so we can't move the old one
                let lines = BufReader::new(&f).lines().enumerate();
                for (current_line_num, line) in lines {
                    let current_line_num = current_line_num.try_into().unwrap();
                    let line = line.unwrap();
                    callback(line, current_line_num);
                }
            }
            Err(err) => {
                log_error!("Error: {:?}", err);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn skip_n_lines(file: &File, n: u64) -> Result<()> {
    let mut lines = BufReader::new(file).lines();
    for i in 0..n {
        let line = lines.next();
        if line.is_none() {
            panic!("File is smaller than the given line number. {} < {}", i, n);
        }
    }
    Ok(())
}

fn watch_file<FFoundUrl, FFoundSeek>(
    log_path: &PathBuf,
    start_after_line: u64,
    mut on_found_url: FFoundUrl,
    mut on_found_seek: FFoundSeek,
) where
    FFoundUrl: FnMut(FoundUrl),
    FFoundSeek: FnMut(FoundSeek),
{
    tail_file(log_path, start_after_line, |line, line_number| {
        if let Some(found_url) = try_match_url_line(&line, line_number) {
            on_found_url(found_url);
        }
        if let Some(found_seek) = try_match_seek_line(&line) {
            on_found_seek(found_seek);
        }
    })
    .expect("Failed to tail file.");
}

pub(crate) struct VrcLogReader {
    log_path: PathBuf,
    lines_read_initially: Option<u64>,
}

impl VrcLogReader {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            log_path: path,
            lines_read_initially: None,
        }
    }

    pub(crate) fn from_latest() -> Self {
        let log_path = get_latest_vrc_log_file().expect("No VRC log files found.");
        Self::new(log_path)
    }

    pub(crate) fn get_latest_url_and_seek(&mut self) -> UrlAndSeekResult {
        if let Some(found_url) = self.find_last_url() {
            if let Some(found_seek) = self.find_last_seek(found_url.found_url_on_line) {
                return UrlAndSeekResult::UrlAndSeek(
                    found_url,
                    found_seek,
                    self.lines_read_initially.unwrap(),
                );
            }
            return UrlAndSeekResult::Url(found_url, self.lines_read_initially.unwrap());
        }

        UrlAndSeekResult::Nothing(self.lines_read_initially.unwrap())
    }

    fn find_last_url(&mut self) -> Option<FoundUrl> {
        let log_path = &self.log_path;
        let mut last_url: Option<FoundUrl> = None;
        log_debug!("Log file: {:#?}", log_path);
        // read log file and look for the last line that matches the regex
        // we must stream the log file as it can be very large
        let file = File::open(log_path).expect("Expected log file to exist.");
        // we should go through the lines forwards, because even though we want the last video url and could exit early if we find it,
        // it's likely that all or most URLs will be toward the beginning of the file
        let lines = BufReader::new(file).lines();
        let mut line_count: u64 = 0;
        for line in lines {
            if let Ok(line) = line {
                line_count += 1;
                if line_count % 100000 == 0 {
                    log_debug!("Processed {} lines.", line_count);
                }

                if let Some(found_url) = try_match_url_line(&line, line_count) {
                    last_url = Some(found_url);
                }
            }
        }
        self.lines_read_initially = Some(line_count);
        last_url
    }

    fn find_last_seek(&mut self, not_before_this_line: u64) -> Option<FoundSeek> {
        let log_path = &self.log_path;
        let mut last_seek: Option<FoundSeek> = None;
        let file = File::open(log_path).expect("Expected log file to exist.");
        let lines = BufReader::new(file).lines();
        let mut line_count: u64 = 0;
        for line in lines {
            if let Ok(line) = line {
                line_count += 1;
                if line_count < not_before_this_line {
                    continue;
                }

                if let Some(found_seek) = try_match_seek_line(&line) {
                    last_seek = Some(found_seek);
                }
            }
        }
        last_seek
    }
}

pub(crate) enum UrlAndSeekResult {
    Nothing(u64),
    Url(FoundUrl, u64),
    UrlAndSeek(FoundUrl, FoundSeek, u64),
}

pub(crate) struct VrcLogWatcher {
    log_path: PathBuf,
}

impl VrcLogWatcher {
    fn new(path: PathBuf) -> Self {
        Self { log_path: path }
    }

    pub(crate) fn from_latest() -> Self {
        let log_path = get_latest_vrc_log_file().expect("No VRC log files found.");
        Self::new(log_path)
    }

    pub(crate) fn watch_file<FFoundUrl, FFoundSeek>(
        &mut self,
        start_after_line: u64,
        on_found_url: FFoundUrl,
        on_found_seek: FFoundSeek,
    ) where
        FFoundUrl: FnMut(FoundUrl),
        FFoundSeek: FnMut(FoundSeek),
    {
        watch_file(
            &self.log_path,
            start_after_line,
            on_found_url,
            on_found_seek,
        );
    }
}

pub(crate) struct FoundSeek {
    pub(crate) timestamp: DateTime<Local>,
    pub(crate) seek_offset: f64,
}

pub(crate) struct FoundUrl {
    pub(crate) timestamp: DateTime<Local>,
    pub(crate) url: String,
    found_url_on_line: u64,
}

pub(crate) enum VrcLogWatcherEvent {
    FoundUrl(FoundUrl),
    FoundSeek(FoundSeek),
}