use core::fmt;
use std::cmp::Ordering::Equal;
use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use yt_dlp::model::format::{Format, FormatType};
use yt_dlp::Downloader;
use yt_dlp::download::DownloadStatus;
use yt_dlp::utils::validation::sanitize_filename;

type BoxError = Box<dyn Error>;

// Below this size, a downloaded video file is almost certainly not real
// video data (some YouTube formats, are
// occasionally throttled/restricted and yt-dlp saves a truncated or
// error response as a video/audio stream instead of raising an error).
const MIN_VALID_VIDEO_BYTES: u64 = 100 * 1024;
const MIN_VALID_AUDIO_BYTES: u64 = 10 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Youtube,
    Instagram,
    Twitter,
    Facebook,
    Other,
}
impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Youtube => write!(f, "Youtube"),
            Platform::Instagram => write!(f, "Instagram"),
            Platform::Twitter => write!(f, "Twitter/X"),
            Platform::Facebook => write!(f, "Facebook"),
            Platform::Other => write!(f, "Other"),
        }
    }
}

//Extract host from url
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_lowercase()
}
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{}",domain))
}

//Detect platform form url host
fn detect_platform(url: &str) -> Platform {
    let host = host_of(url);
    if ["youtube.com", "youtu.be", "youtube-nocookie.com"].iter().any(|d| host_matches(&host, d)) {
        Platform::Youtube
    }else if host_matches(&host, "instagram.com") {
        Platform::Instagram
    }else if host_matches(&host, "twitter.com") || host_matches(&host, "x.com") {
        Platform::Twitter
    } else if ["facebook.com", "fb.com", "fb.watch"].iter().any(|d| host_matches(&host, d))
    {
        Platform::Facebook
    } else {
        Platform::Other
    }
}


//Format directly downlodable only if it is not a manifest/HLS playlist
fn is_direct(format: &Format) -> bool {
    format.download_info.manifest_url.is_none()
}

//Remove a file at `path` if it already exists, so a later write starts clean.
// fn clear_existing(path: &Path) {
//     if path.exists() {
//         if let Err(err) = std::fs::remove_file(path) {
//             eprintln!("Warning: could not remove existing file {:?} before overwriting ({})", path, err);
//         }
//     }
// }

//
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|e| e.to_str());
    let parent = path.parent().unwrap_or_else(|| Path::new(""));

    let mut n: u32 = 1;
    loop {
        let file_name = match ext {
            Some(ext) => format!("{} ({}).{}", stem, n, ext),
            None => format!("{} ({})", stem, n),
        };
        let candidate = parent.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

//Print an in-place progress bar for a download.
fn print_progress(label: &str, downloaded: u64, total: u64) {
    const WIDTH: usize = 30;
    let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);

    if total > 0 {
        let ratio = (downloaded as f64 / total as f64).clamp(0.0, 1.0);
        let filled = (ratio * WIDTH as f64).round()as usize;
        let bar = format!("{}{}", "=".repeat(filled), " ".repeat(WIDTH - filled));
        print!(
            "\r{:<28} [{}] {:>3}% ({:.1}/{:.1} MB)   ",
            label,
            bar,
            (ratio * 100.0) as u32,
            mb(downloaded),
            mb(total)
        );
    }else {
        print!("\r{:<28} downloaded {:.1} MB   ", label, mb(downloaded));
    }
    let _ = io::stdout().flush();
}

//Builds a progressive callback tied to a specific display label
fn progress_callback_for(label: String) -> impl Fn(u64, u64) + Send + Sync + 'static {
    move | downloaded, total| print_progress(&label, downloaded, total)
}

//Prompt to type a line and return the trimmed result.
fn prompt_line(prompt: &str) -> io::Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
 
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

//Prompt for a menu choice
fn prompt_choice(count: usize) -> Option<usize> {
    println!();
    print!("Enter choice: ");
    io::stdout().flush().ok()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice).ok()?;
    let choice: usize = choice.trim().parse().ok()?;
    println!();

    if choice == 0 || choice > count {
        return None;
    }
    Some(choice)
}


//Convert a `watch?v=` style YouTube URL into the shorter `youtu.be` form.
fn normalize_youtube_url(url_input: &str) -> Result<String, BoxError> {
    if let Some(video_id) = url_input
        .strip_prefix("https://www.youtube.com/watch?v=")
        .map(|rest| rest.split('&').next().unwrap_or(rest))
    {
        Ok(format!("https://youtu.be/{}", video_id))
    } else {
        Ok(url_input.to_string())
    }
}

//file extension reported by the format, falling back to default when unknown
fn format_extension<'a>(format: &Format, default: &'a str) -> &'a str {
    match format.download_info.ext.as_str() {
        "bin" => default,
        ext => ext,
    }
}


//From a list of audio formats, pick the best one
//prefer formats explicitly marked "original", then pick the highest bitrate/quality
//or fall back to the next best option if current one is broken
fn rank_audio_formats<'a>(audio_formats: &[&'a Format]) -> Vec<&'a Format> {
    let exp_original: Vec<&Format> = audio_formats
        .iter()
        .filter(|format| {
            format
                .format_note
                .as_deref()
                .map(|note| note.to_lowercase().contains("original"))
                .unwrap_or(false)
        })
        .copied()
        .collect();
 
    let mut candidates = if !exp_original.is_empty() {
        exp_original
    } else {
        audio_formats.to_vec()
    };
 
    candidates.sort_by(|a, b| {
        let a_rate = a.rates_info.audio_rate.unwrap_or_default();
        let b_rate = b.rates_info.audio_rate.unwrap_or_default();
        b_rate
            .partial_cmp(&a_rate)
            .unwrap_or(Equal)
            .then_with(|| {
                let a_quality = a.quality_info.quality.unwrap_or_default();
                let b_quality = b.quality_info.quality.unwrap_or_default();
                b_quality.partial_cmp(&a_quality).unwrap_or(Equal)
            })
    });
    candidates
}

//Collect distinct video heights (resolutions), sorted from highest to lowest.
fn available_resolutions(watchable: &Vec<&Format>) -> Vec<u32> {
    let mut resolutions: Vec<u32> = watchable
        .iter()
        .filter_map(|format| format.video_resolution.height)
        .filter(|height| *height > 0)
        .collect();
 
    resolutions.sort_unstable();
    resolutions.dedup();
    resolutions.reverse();
    resolutions
}

//Pick the highest quality video
//and gives callers a fallback list
fn rank_video_formats<'a>(formats: &[&'a Format]) -> Vec<&'a Format> {
    let mut candidates: Vec<&Format> = formats.to_vec();
 
    candidates.sort_by(|a, b| {
        let a_quality = a.quality_info.quality.unwrap_or_default();
        let b_quality = b.quality_info.quality.unwrap_or_default();
        b_quality
            .partial_cmp(&a_quality)
            .unwrap_or(Equal)
            .then_with(|| {
                let a_rate = a.rates_info.total_rate.unwrap_or_default();
                let b_rate = b.rates_info.total_rate.unwrap_or_default();
                b_rate.partial_cmp(&a_rate).unwrap_or(Equal)
            })
    });
 
    candidates
}


//Downloading with progress and checking the result if its a valid video
async fn download_first_valid<'a>(
    downloader: &Downloader,
    candidates: &[&'a Format],
    destination_for: impl Fn(&Format) -> PathBuf,
    min_size_byte: u64,
) -> Result<(PathBuf, &'a Format), BoxError> {
    let mut last_error: Option<BoxError> = None;
    let manager = downloader.download_manager();

    for format in candidates {
        let destination = destination_for(*format);

        let url = format.url()?.clone();
        let headers = Some(format.download_info.http_headers.clone());
        let label = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download")
            .to_string();

        let download_id = manager
            .enqueue_with_progress_and_headers(
                &url,
                destination.clone(),
                None,
                progress_callback_for(label),
                headers,
            ).await;
        let status = manager.wait_for_completion(download_id).await;
        println!();

        match status {
            Some(DownloadStatus::Completed) => match std::fs::metadata(&destination) {
                Ok(meta) if meta.len() >= min_size_byte => return Ok((destination, *format)),
                Ok(meta) => {
                    eprintln!("Format {} downloaded but looks invalid ({} bytes) - likely throtteled or restricted by yt, trying next option...",
                        format.format_id, meta.len()
                    );
                    let _ = std::fs::remove_file(&destination);
                    last_error = Some(format!("format {} produced an invalid file", format.format_id).into());
                }
                Err(err) => {
                    last_error = Some(Box::new(err));
                }
            },
            Some(DownloadStatus::Failed { reason }) => {
                eprintln!("Format {} failed to download ({}), trying next option...", format.format_id, reason);
                last_error = Some(reason.into());
            }
            Some(DownloadStatus::Canceled) => {
                eprintln!("Format {} download was canceled, trying next option...", format.format_id);
                last_error = Some(format!("format {} download was canceled", format.format_id).into());
            }
            other => {
                eprintln!("Format {} ended in an unexpected state ({:?}), trying next option...", format.format_id, other);
                last_error = Some(format!("format {} ended in an unexpected state", format.format_id).into());
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "No working format found".into()))
}

//Download audio only or
//fall back through `audio_candidates` if best one is bad
async fn download_audio_only(
    downloader: &Downloader,
    audio_formats: &[&Format],
    progressive_formats: &[&Format],
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<(), BoxError> {
    let audio_candidates = rank_audio_formats(audio_formats);
    if !audio_candidates.is_empty() {
        println!("Audio: {} - {}", audio_candidates[0]. format_id, audio_candidates[0].format_note.as_deref().unwrap_or("Unknown"));
        println!();
        println!("Downloading audio only...");

        let (audio_path, used) = download_first_valid(
            downloader,
            &audio_candidates,
            |candidate| unique_path(&downloads_dir.join(format!("{}.{}", sanitized_title, format_extension(candidate, "m4a")))),
            MIN_VALID_AUDIO_BYTES,
        ).await?;

        println!("Used audio format: {}", used.format_id);
        println!("Audio saved to: {:?}", audio_path);
        return Ok(());
    }
    //No separate audio tracks: fallback to best progressive file
    let progressive_candidates = rank_video_formats(progressive_formats);
    if !progressive_candidates.is_empty() {
        println!("No separate audio track; downloading best combined file instead.");
        let (path, used) = download_first_valid(
            downloader,
            &progressive_candidates,
            |candidate| unique_path(&downloads_dir.join(format!("{}.{}", sanitized_title, format_extension(candidate, "mp4")))),
            MIN_VALID_VIDEO_BYTES,
        ).await?;

        println!("Used format: {}", used.format_id);
        println!("Audio saved to: {:?}", path);
        println!();
        return Ok(());
    }
    Err("No downloadable audio track found".into())
}

//Download video and audio and merging
async fn download_video_with_audio(
    downloader: &Downloader,
    progressive_formats: &[&Format],
    video_only_formats: &[&Format],
    audio_formats: &[&Format],
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<(), BoxError> {
    //All downloadable formats that carry a picture, for res selection
    let watchable: Vec<&Format> = progressive_formats
        .iter()
        .chain(video_only_formats.iter())
        .copied()
        .filter(|format| {
            format.video_resolution.height.is_some_and(|h| h > 0) && format.video_resolution.width.is_some()
        })
        .collect();
    
    let resolutions = available_resolutions(&watchable);
    if resolutions.is_empty() {
        return Err("No video formats found".into());
    }
    
    println!();
    println!("Choose video quality: ");
    for (index, resolution) in resolutions.iter().enumerate() {
        println!("{}. {}p", index+1, resolution);
    }
    let choice = prompt_choice(resolutions.len()).ok_or("Invalid choice")?;
    let selectd_height = resolutions[choice - 1];

    let at_height: Vec<&Format> = watchable
        .iter()
        .copied()
        .filter(|format| format.video_resolution.height == Some(selectd_height))
        .collect();

    //Preference for a progressive file: no merge needed.
    let progressive_at_height: Vec<&Format> = at_height
        .iter()
        .copied()
        .filter(|format| format.format_type() == FormatType::AudioVideo)
        .collect();
    let progressive_candidates = rank_video_formats(&progressive_at_height);

    if !progressive_candidates.is_empty() {
        println!(
            "Video format: {} - {} (progressive, no merge needed)",
            progressive_candidates[0].format_id,
            progressive_candidates[0]
                .video_resolution
                .resolution
                .as_deref()
                .unwrap_or("unknown")
        );
        println!("Downloading Video...");

        let (path, used) = download_first_valid(
            downloader,
            &progressive_candidates,
            |candidate| unique_path(&downloads_dir.join(format!("{}.{}", sanitized_title, format_extension(candidate, "mp4")))),
            MIN_VALID_VIDEO_BYTES,
        ).await?;
        println!("Used format: {}", used.format_id);
        println!("Video saved to: {:?}", path);
        return Ok(());
    }

    //Fallback to seperate video and audio streams merged with ffmpeg
    let video_only_at_height: Vec<&Format> = at_height
        .iter()
        .copied()
        .filter(|format| format.format_type() == FormatType::Video)
        .collect();
    let video_candidates = rank_video_formats(&video_only_at_height);
    if video_candidates.is_empty(){
        return Err("Could not find video format".into());
    }
    let audio_candidates = rank_audio_formats(audio_formats);
    if audio_candidates.is_empty(){
        return Err("No downloadable audio track found".into());
    }

    // Temporary Files
    let temp_dir = std::env::temp_dir();
    let video_temp = temp_dir.join("video_stream.mp4");
    let audio_temp = temp_dir.join("audio_stream.m4a");
    let video_destination = unique_path(&downloads_dir.join(format!("{}.mp4", sanitized_title)));
 
    println!("Downloading video...");
    let (video_path, used_video) = download_first_valid(
        downloader,
        &video_candidates,
        |_| video_temp.clone(),
        MIN_VALID_VIDEO_BYTES
    ).await?;

    println!(
        "Video stream downloaded (format {} - {})",
        used_video.format_id,
        used_video.video_resolution.resolution.as_deref().unwrap_or("unknown")
    );

    println!();
    println!("Downloading audio...");
    let (audio_path, used_audio) = download_first_valid(
        downloader,
        &audio_candidates, 
        |_| audio_temp.clone(), 
        MIN_VALID_AUDIO_BYTES
    ).await?;

    println!(
        "Audio stream downloaded (format {})",
        used_audio.format_id
    );


    println!();
    println!("Combining video and audio...");
    let final_path = downloader
        .combine_audio_and_video_to_path(audio_path, video_path, &video_destination)
        .await?;
    println!("Video saved to: {:?}", final_path);
    println!();

    // Clean up temporary files only (preserve libs)
    let _ = std::fs::remove_file(&video_temp);
    let _ = std::fs::remove_file(&audio_temp);
    let _ = std::fs::remove_dir_all("output");
 
    Ok(())
}

//The parsed contents of a line typed at the main prompt
struct UserCommand {
    url: String,
    audio_only: bool,
}

//Parse a raw input line into a normalized YouTube URL plus any flag
fn parse_user_command(input_line: &str) -> Result<UserCommand, BoxError> {
    let mut tokens = input_line.split_whitespace();
    let url_input = tokens.next().unwrap_or_default();
    let audio_only = tokens.any(|arg| arg == "--audio-only");
 
    let url = normalize_youtube_url(url_input)?;
    Ok(UserCommand { url, audio_only })
}


//Handle url, fetch info, pick formats and download
async fn process_url(
    downloader: &Downloader,
    command: UserCommand,
) -> Result<(), BoxError> {
    let url_input = command.url.as_str();
    let platform = detect_platform(url_input);
    println!("Detected platform: {}", platform);

    let normalized_url = normalize_youtube_url(url_input)?;

    //Fetching video Information
    println!("Fetching Video Information");
    let video = downloader.fetch_video_infos_fresh(normalized_url).await?;
    println!("VIDEO TITLE: {}", video.title);
    
    let downloads_dir = dirs::download_dir().ok_or("Could not find download directory.")?;
    let sanitized_title = sanitize_filename(&video.title);
 
    // Classify every direct (non-HLS/manifest) format by its actual codec
    // content (acodec/vcodec presence) rather than any display-only string field
    let audio_formats: Vec<&Format> = video
        .formats
        .iter()
        .filter(|format| is_direct(format) && format.format_type() == FormatType::Audio)
        .collect();

    let progressive_formats: Vec<&Format> = video
        .formats
        .iter()
        .filter(|format| is_direct(format) && format.format_type() == FormatType::AudioVideo)
        .collect();

    let video_only_formats: Vec<&Format> = video
        .formats
        .iter()
        .filter(|format| is_direct(format) && format.format_type() == FormatType::Video)
        .collect();

 
    if command.audio_only {
        return download_audio_only(downloader, &audio_formats, &progressive_formats, &downloads_dir, &sanitized_title)
            .await;
    }

    download_video_with_audio(
        downloader,
        &progressive_formats,
        &video_only_formats,
        &audio_formats,
        &downloads_dir,
        &sanitized_title,
    ).await
}

//Downlaods the binaries
async fn setup_bins_downloader() -> Result<Downloader, BoxError> {
    let output_dir = PathBuf::from("output");
    let libraries_dir = PathBuf::from("libs");
 
    println!("Installing binaries...");
    let downloader = Downloader::with_new_binaries(libraries_dir, output_dir)
        .await?
        .build()
        .await?;
    println!("Libraries ready.");
    println!();
 
    Ok(downloader)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   let downloader = setup_bins_downloader().await?;
   println!("Supported: YouTube, Instagram, Twitter/X, Facebook (and other sites).");

    loop {
        let input_line = prompt_line("Enter URL [--audio-only] (or 'exit', 'quit'): ")?;
 
        if input_line.is_empty()
            || input_line.eq_ignore_ascii_case("exit")
            || input_line.eq_ignore_ascii_case("quit")
        {
            break;
        }
 
        let command = match parse_user_command(&input_line) {
            Ok(command) => command,
            Err(err) => {
                eprintln!("{}", err);
                continue;
            }
        };
 
        if let Err(err) = process_url(&downloader, command).await {
            eprintln!("{}", err);
            continue;
        }
    }
 
    println!("Goodbye!! UwU...");
    thread::sleep(Duration::from_secs(2));
    Ok(())
}