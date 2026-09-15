use std::cmp::Ordering::Equal;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use yt_dlp::Downloader;

type BoxError = Box<dyn Error>;

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

fn prompt_line(prompt: &str) -> io::Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
 
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}


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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = PathBuf::from("output");

    // Install bins: yt-dlp, ffmpeg (skipped if already present).
    println!("Installing binaries...");
    let libraries_dir = PathBuf::from("libs");
    let downloader = Downloader::with_new_binaries(
        libraries_dir,
        output_dir
    ).await?.build().await?;
    println!("Libraries ready.");
    println!();

    loop {
        let input_line = prompt_line("Enter URL [--audio-only] (or 'exit', 'quit'): ")?;

        if input_line.is_empty() || input_line.eq_ignore_ascii_case("exit") || input_line.eq_ignore_ascii_case("quit") {
            break;
        }


        // Parse URL and optional args (e.g. --audio-only).
        let command = match parse_user_command(&input_line) {
            Ok(command) => command,
            Err(err) => {
                eprintln!("{}", err);
                continue;
            }
        };

        // Fetching video information.
        println!("Fetching Video Information");
        let video = downloader.fetch_video_infos_fresh(command.url).await?;
        println!("VIDEO TITLE: {}", video.title);

        // Original audio

        // Finding available audio formats
        let audio_formats: Vec<_> = video
            .formats
            .iter()
            .filter(|format| {
                let is_audio_only = format
                    .video_resolution
                    .resolution
                    .as_deref() == Some("audio only");

                // Filter out HLS/M3U8 streams
                let is_not_hls = format
                    .download_info
                    .manifest_url
                    .is_none();
                is_audio_only && is_not_hls
            }).collect();

        // Error if no audio format found
        if audio_formats.is_empty(){
            eprintln!("No non-HLS audio track found");
            continue;
        }

        // List of explicitly marked "original"
        let exp_original: Vec<_> = audio_formats
            .iter()
            .filter(|format| {
                format
                    .format_note
                    .as_deref()
                    .map(|note| note.to_lowercase().contains("original"))
                    .unwrap_or(false)
            }).copied().collect();

        // Prefer original
        let candidates = if !exp_original.is_empty(){
            exp_original
        }else{
            audio_formats.clone()
        };

        // Select highest quality audio
        let original_audio = candidates
            .into_iter()
            .max_by(|a, b| {
                let a_rate = a.rates_info.audio_rate.unwrap_or_default();
                let b_rate = b.rates_info.audio_rate.unwrap_or_default();
                a_rate.partial_cmp(&b_rate)
                    .unwrap_or(Equal)
                    .then_with(|| {
                        let a_quality = a.quality_info.quality.unwrap_or_default();
                        let b_quality = b.quality_info.quality.unwrap_or_default();
                        a_quality
                            .partial_cmp(&b_quality)
                            .unwrap_or(Equal)
                    })
            }).ok_or("Could not select an audio format")?;

        println!("Original audio: {} - {}",
            original_audio.format_id,
            original_audio.format_note.as_deref().unwrap_or("unknown")
        );


        //Option for audio only.
        let downloads_dir = dirs::download_dir()
            .ok_or("Could not determine the downloads directory")?;
        let sanitized_title = sanitize_filename(&video.title);
        // Audio-only download.
        if command.audio_only {
            let audio_destination = downloads_dir.join(format!("{}.m4a", sanitized_title));
            println!();
            println!("Downloading audio only...");
            let audio_path = downloader
                .download_format(original_audio, audio_destination.to_str().unwrap()).await?;

            println!("Audio saved to: {:?}", audio_path);
            println!();
            continue;
        }


        // Video quality menu

        // Collect all available video resolutions.
        let mut resolutions: Vec<u32> = video
            .formats
            .iter()
            .filter_map(|format| {
                format.video_resolution.height
            })
            .filter(|height| *height > 0)
            .collect();

        // Remove duplicate resolutions and sort from highest to lowest.
        resolutions.sort_unstable();
        resolutions.dedup();
        resolutions.reverse();
        if resolutions.is_empty(){
            eprintln!("No video formats found");
            continue;
        }

        // Choose quality
        println!();
        println!("Choose video quality: ");
        for (index, resolution) in resolutions.iter().enumerate(){
            println!("{}. {}p", index+1, resolution);
        }

        let choice = prompt_choice(resolutions.len()).ok_or("Invalid choice")?;

        println!();

        let selected_height = resolutions[choice-1];

        // Find the best video format at selected resolution.
        let video_format = video
            .formats
            .iter()
            .filter(|format| {
                format.video_resolution.height == Some(selected_height)
                && format.video_resolution.width.is_some()
            })
            .max_by(|a, b| {
                let a_quality = a.quality_info.quality.unwrap_or_default();
                let b_quality = b.quality_info.quality.unwrap_or_default();
                a_quality
                    .partial_cmp(&b_quality)
                    .unwrap_or(Equal)
            }).ok_or("Could not find selected video format")?;

        println!("Video format: {} - {}",
            video_format.format_id,
            video_format
                .video_resolution
                .resolution
                .as_deref()
                .unwrap_or("unknown")
        );

        // Temporary files in system temp directory
        let temp_dir = std::env::temp_dir();
        let video_temp = temp_dir.join("video_stream.mp4");
        let audio_temp = temp_dir.join("audio_stream.m4a");

        // Final output in OS downloads directory with video title
        let video_destination = downloads_dir.join(format!("{}.mp4", sanitized_title));

        // Download video stream
        println!("Downloading video...");

        let video_path = downloader
            .download_format(video_format, video_temp.to_str().unwrap()).await?;
        println!("Video stream downloaded: {:?}", video_path);

        // Download audio stream
        println!("Downloading audio...");

        let audio_path = downloader
            .download_format(original_audio, audio_temp.to_str().unwrap()).await?;
        println!("Audio stream downloaded: {:?}", audio_path);

        // Combine video and audio
        println!("Combining video and original audio...");

        let final_path = downloader
            .combine_audio_and_video_to_path(
                audio_path,
                video_path,
                &video_destination,
            ).await?;

        println!("Video saved to: {:?}", final_path);

        // Clean up temporary files only (preserve libs and output directories)
        let _ = std::fs::remove_file(&video_temp);
        let _ = std::fs::remove_file(&audio_temp);
        let _ = std::fs::remove_dir_all("output");

        println!();
    }

    println!("Goodbye!! UwU...");
    thread::sleep(Duration::from_secs(2));
    Ok(())
}