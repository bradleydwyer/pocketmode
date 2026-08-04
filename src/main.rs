use pocketmode::{Detection, inspect_file};
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = r#"PocketMode — identify DJI Osmo Pocket 4P recording modes

USAGE:
    pocketmode [OPTIONS] [PATH]...
    pocketmode help

With no PATH, PocketMode scans supported video files in the current directory.
A PATH may be an individual file or a directory. Directory scans are shallow
unless --recursive is supplied.

OPTIONS:
    -r, --recursive    Recurse into directories
    -j, --json         Emit machine-readable JSON
    -v, --verbose      Include the evidence used for each classification
    -h, --help         Print this help
    -V, --version      Print the version

SUPPORTED DIRECTORY-SCAN EXTENSIONS:
    .mp4, .mov, .m4v

EXAMPLES:
    pocketmode
    pocketmode ~/Video
    pocketmode clip1.MP4 clip2.MP4
    pocketmode --recursive --json /Volumes/CAMERA
"#;

#[derive(Debug, Default)]
struct Cli {
    recursive: bool,
    json: bool,
    verbose: bool,
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("pocketmode: {message}");
            eprintln!("Try 'pocketmode help' for usage.");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<OsString>) -> Result<ExitCode, String> {
    if args.len() == 1 && args[0] == "help" {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }

    let cli = parse_args(args)?;
    let inputs = if cli.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.paths.clone()
    };

    let mut files = BTreeSet::new();
    let mut errors = Vec::new();
    for input in inputs {
        collect_input(&input, cli.recursive, &mut files, &mut errors);
    }

    if files.is_empty() {
        if cli.json {
            print_json(&[], &errors);
        } else {
            for error in &errors {
                eprintln!("pocketmode: {error}");
            }
            eprintln!("pocketmode: no supported video files found");
        }
        return Ok(ExitCode::FAILURE);
    }

    let mut results = Vec::new();
    for path in files {
        match inspect_file(&path) {
            Ok(detection) => results.push((path, detection)),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }

    if cli.json {
        print_json(&results, &errors);
    } else {
        print_table(&results, cli.verbose);
        for error in &errors {
            eprintln!("pocketmode: {error}");
        }
    }

    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn parse_args(args: Vec<OsString>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut positional_only = false;

    for argument in args {
        if positional_only {
            cli.paths.push(argument.into());
            continue;
        }

        let text = argument.to_string_lossy();
        match text.as_ref() {
            "--" => positional_only = true,
            "-r" | "--recursive" => cli.recursive = true,
            "-j" | "--json" => cli.json = true,
            "-v" | "--verbose" => cli.verbose = true,
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("pocketmode {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "help" => return Err("'help' must be used on its own".to_owned()),
            _ if text.starts_with('-') => return Err(format!("unknown option: {text}")),
            _ => cli.paths.push(argument.into()),
        }
    }

    Ok(cli)
}

fn collect_input(
    path: &Path,
    recursive: bool,
    files: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            files.insert(path.to_owned());
        }
        Ok(metadata) if metadata.is_dir() => collect_directory(path, recursive, files, errors),
        Ok(_) => errors.push(format!(
            "{} is not a regular file or directory",
            path.display()
        )),
        Err(error) => errors.push(format!("{}: {error}", path.display())),
    }
}

fn collect_directory(
    directory: &Path,
    recursive: bool,
    files: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{}: {error}", directory.display()));
            return;
        }
    };

    let mut entries: Vec<_> = entries.collect();
    entries.sort_by_key(|entry| entry.as_ref().ok().map(|entry| entry.path()));

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };

        if file_type.is_file() && is_supported_video(&path) {
            files.insert(path);
        } else if file_type.is_dir() && recursive {
            collect_directory(&path, true, files, errors);
        } else if file_type.is_symlink() {
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() && is_supported_video(&path) => {
                    files.insert(path);
                }
                Ok(_) => {}
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
    }
}

fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp4" | "mov" | "m4v"
            )
        })
}

fn print_table(results: &[(PathBuf, Detection)], verbose: bool) {
    if verbose {
        println!(
            "{:<10}  {:<10}  {:<64}  FILE",
            "MODE", "CONFIDENCE", "EVIDENCE"
        );
        for (path, detection) in results {
            println!(
                "{:<10}  {:<10}  {:<64}  {}",
                detection.mode.as_str(),
                detection.confidence.as_str(),
                detection.evidence,
                path.display()
            );
        }
    } else {
        println!("{:<10}  {:<10}  FILE", "MODE", "CONFIDENCE");
        for (path, detection) in results {
            println!(
                "{:<10}  {:<10}  {}",
                detection.mode.as_str(),
                detection.confidence.as_str(),
                path.display()
            );
        }
    }
}

fn print_json(results: &[(PathBuf, Detection)], errors: &[String]) {
    println!("{{");
    println!("  \"results\": [");
    for (index, (path, detection)) in results.iter().enumerate() {
        let comma = if index + 1 == results.len() { "" } else { "," };
        println!("    {{");
        println!("      \"file\": {},", json_string(&path.to_string_lossy()));
        println!("      \"mode\": {},", json_string(detection.mode.as_str()));
        println!(
            "      \"confidence\": {},",
            json_string(detection.confidence.as_str())
        );
        println!("      \"evidence\": {},", json_string(&detection.evidence));
        println!("      \"fingerprints\": {{");
        println!(
            "        \"color_gamma_key\": {},",
            detection.fingerprints.color_gamma_key
        );
        println!("        \"d_log_2\": {},", detection.fingerprints.d_log_2);
        println!("        \"d_log_m\": {},", detection.fingerprints.d_log_m);
        println!("        \"d_log\": {},", detection.fingerprints.d_log);
        println!("        \"normal\": {},", detection.fingerprints.normal);
        println!("        \"hlg\": {},", detection.fingerprints.hlg);
        println!("        \"de_gamma\": {},", detection.fingerprints.de_gamma);
        println!(
            "        \"yuv_gamma\": {}",
            detection.fingerprints.yuv_gamma
        );
        println!("      }}");
        println!("    }}{comma}");
    }
    println!("  ],");
    println!("  \"errors\": [");
    for (index, error) in errors.iter().enumerate() {
        let comma = if index + 1 == errors.len() { "" } else { "," };
        println!("    {}{comma}", json_string(error));
    }
    println!("  ]");
    println!("}}");
}

fn json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_video_extensions_case_insensitively() {
        assert!(is_supported_video(Path::new("clip.MP4")));
        assert!(is_supported_video(Path::new("clip.mov")));
        assert!(is_supported_video(Path::new("clip.M4V")));
        assert!(!is_supported_video(Path::new("clip.txt")));
    }

    #[test]
    fn parses_paths_and_options() {
        let cli = parse_args(vec!["--recursive".into(), "--json".into(), "clips".into()]).unwrap();

        assert!(cli.recursive);
        assert!(cli.json);
        assert_eq!(cli.paths, vec![PathBuf::from("clips")]);
    }

    #[test]
    fn rejects_unknown_options() {
        assert!(parse_args(vec!["--wat".into()]).is_err());
    }

    #[test]
    fn escapes_json_strings() {
        assert_eq!(json_string("a\n\"b"), "\"a\\n\\\"b\"");
    }
}
