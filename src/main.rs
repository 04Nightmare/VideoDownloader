use std::cmp::Ordering::Equal;
use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use yt_dlp::model::format::Format;
use yt_dlp::model::Video;
use yt_dlp::Downloader;

type BoxError = Box<dyn Error>;

const MIN_VALID_VIDEO_BYTES: u64 = 100 * 1024;

const MIN_VALID_AUDIO_BYTES: u64 = 10 * 1024;

//Replace character that isn't alphanumeric, space, dash or underscore
fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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

//Return every non-HLS, audio-only format available for a video.
fn find_audio_formats(video: &Video) -> Vec<&Format> {
    video
        .formats
        .iter()
        .filter(|format| {
            let is_audio_only = format.video_resolution.resolution.as_deref() == Some("audio only");
            let is_not_hls = format.download_info.manifest_url.is_none();
            is_audio_only && is_not_hls
        })
        .collect()
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
        a_rate
            .partial_cmp(&b_rate)
            .unwrap_or(Equal)
            .then_with(|| {
                let a_quality = a.quality_info.quality.unwrap_or_default();
                let b_quality = b.quality_info.quality.unwrap_or_default();
                a_quality.partial_cmp(&b_quality).unwrap_or(Equal)
            })
    });
    candidates
}

//Collect distinct video heights (resolutions), sorted from highest to lowest.
fn available_resolutions(video: &Video) -> Vec<u32> {
    let mut resolutions: Vec<u32> = video
        .formats
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
fn rank_video_formats(video: &Video, height: u32) -> Vec<&Format> {
    let mut candidates: Vec<&Format> = video
        .formats
        .iter()
        .filter(|format| {
            format.video_resolution.height == Some(height) && format.video_resolution.width.is_some()
        })
        .collect();
 
    candidates.sort_by(|a, b| {
        let a_quality = a.quality_info.quality.unwrap_or_default();
        let b_quality = b.quality_info.quality.unwrap_or_default();
        b_quality.partial_cmp(&a_quality).unwrap_or(Equal)
    });
 
    candidates
}

//Choose video quality menu
fn choose_video_format<'a>(video: &'a Video) -> Result<Vec<&'a Format>, BoxError> {
    let resolutions = available_resolutions(video);
    if resolutions.is_empty() {
        return Err("No video formats found".into());
    }
 
    println!();
    println!("Choose video quality: ");
    for (index, resolution) in resolutions.iter().enumerate() {
        println!("{}. {}p", index + 1, resolution);
    }
 
    let choice = prompt_choice(resolutions.len()).ok_or("Invalid choice")?;
    let selected_height = resolutions[choice-1];

    let candidates = rank_video_formats(video, selected_height);
    if candidates.is_empty(){
        return Err("Could not find selected video fromat".into());
    }
    Ok(candidates)
}

//Downloading and checking the result if its a valid video
async fn download_first_valid<'a>(
    downloader: &Downloader,
    candidates: &[&'a Format],
    destination: &str,
    min_size_byte: u64,
) -> Result<(PathBuf, &'a Format), BoxError> {
    let mut last_error: Option<BoxError> = None;
    for format in candidates {
        match downloader.download_format(*format, destination).await {
            Ok(path) => match std::fs::metadata(&path) {
                Ok(meta) if meta.len() >= min_size_byte => return Ok((path, *format)),
                Ok(meta) => {
                    eprintln!("Format {} downloaded but looks invalid ({} bytes) - likely throtteled or restricted by yt, trying next option...",
                        format.format_id, meta.len()
                    );
                    let _ = std::fs::remove_file(&path);
                    last_error = Some(format!("format {} produced an invalid file", format.format_id).into());
                }
                Err(err) => {
                    last_error = Some(Box::new(err));
                }
            },
            Err(err) => {
                eprintln!("Format {} failed to download ({}), trying next option...", format.format_id, err);
                last_error = Some(err.into());
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "No working format found".into()))
}

//Download audio only or
//fall back through `audio_candidates` if best one is bad
async fn download_audio_only(
    downloader: &Downloader,
    audio_candidates: &[&Format],
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<PathBuf, BoxError> {
    let audio_destination = downloads_dir.join(format!("{}.m4a", sanitized_title));
 
    println!();
    println!("Downloading audio only...");
    let (audio_path, used_format) = download_first_valid(
        downloader, 
        audio_candidates,
        audio_destination.to_str().unwrap(),
        MIN_VALID_AUDIO_BYTES
    ).await?;
    println!("Used audio format: {}", used_format.format_id);
    Ok(audio_path)
}

//Download video and audio and merging
async fn download_video_with_audio(
    downloader: &Downloader,
    video_candidates: &[&Format],
    audio_candidates: &[&Format],
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<PathBuf, BoxError> {
    let temp_dir = std::env::temp_dir();
    let video_temp = temp_dir.join("video_stream.mp4");
    let audio_temp = temp_dir.join("audio_stream.m4a");
    let video_destination = downloads_dir.join(format!("{}.mp4", sanitized_title));
 
    println!("Downloading video...");
    let (video_path, used_video_format) = download_first_valid(
        downloader, video_candidates, video_temp.to_str().unwrap(), MIN_VALID_VIDEO_BYTES).await?;
    println!(
        "Video stream downloaded: {:?} (format {} - {})",
        video_path,
        used_video_format.format_id,
        used_video_format.video_resolution.resolution.as_deref().unwrap_or("unknown")
    );

    println!("Downloading audio...");
    let (audio_path, used_audio_format) = download_first_valid(
        downloader, audio_candidates, audio_temp.to_str().unwrap(), MIN_VALID_AUDIO_BYTES).await?;
    println!(
        "Audio stream downloaded: {:?} (format {})",
        audio_path, used_audio_format.format_id
    );
 
    println!("Combining video and original audio...");
    let final_path = downloader
        .combine_audio_and_video_to_path(audio_path, video_path, &video_destination)
        .await?;
 
    // Clean up temporary files only (preserve libs)
    let _ = std::fs::remove_file(&video_temp);
    let _ = std::fs::remove_file(&audio_temp);
    let _ = std::fs::remove_dir_all("output");
 
    Ok(final_path)
}

//Handle url, fetch info, pick formats and download
async fn process_url(
    downloader: &Downloader,
    downloads_dir: &Path,
    command: UserCommand,
) -> Result<(), BoxError> {
    println!("Fetching Video Information");
    let video = downloader.fetch_video_infos_fresh(command.url).await?;
    println!("VIDEO TITLE: {}", video.title);
 
    let audio_formats = find_audio_formats(&video);
    if audio_formats.is_empty() {
        return Err("No non-HLS audio track found".into());
    }
 
    let audio_candidates = rank_audio_formats(&audio_formats);
    let best_audio = *&audio_candidates.first().ok_or("Could not select an audio format")?;
 
    println!(
        "Original audio: {} - {}",
        best_audio.format_id,
        best_audio.format_note.as_deref().unwrap_or("unknown")
    );
 
    let sanitized_title = sanitize_filename(&video.title);
 
    if command.audio_only {
        let audio_path =
            download_audio_only(downloader, &audio_candidates, downloads_dir, &sanitized_title).await?;
        println!("Audio saved to: {:?}", audio_path);
        println!();
        return Ok(());
    }
 
   let video_candidates = choose_video_format(&video)?;
   let final_path = download_video_with_audio(
        downloader,
        &video_candidates, 
        &audio_candidates, 
        downloads_dir, 
        &sanitized_title
    ).await?;
 
    println!("Video saved to: {:?}", final_path);
    println!();
 
    Ok(())
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
    let downloads_dir = dirs::download_dir().ok_or("Could not determine the downloads directory")?;
 
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
 
        if let Err(err) = process_url(&downloader, &downloads_dir, command).await {
            eprintln!("{}", err);
            continue;
        }
    }
 
    println!("Goodbye!! UwU...");
    thread::sleep(Duration::from_secs(2));
    Ok(())
}