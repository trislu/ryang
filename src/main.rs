use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use ryang::Ryang;
use std::fs;
use std::path::Path;

#[derive(Parser)]
#[command(name = "ryang")]
#[command(about = "YANG library CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Input mode
    #[arg(long, default_value = "directory")]
    mode: Mode,

    /// Input paths
    #[arg(value_name = "PATHS")]
    paths: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Display help
    Help,
}

#[derive(Clone, Debug)]
enum Mode {
    Directory,
    File,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "directory" | "dir" => Ok(Mode::Directory),
            "file" => Ok(Mode::File),
            _ => Err(format!("Invalid mode: {}", s)),
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Help) = cli.command {
        Cli::command().print_help()?;
        return Ok(());
    }

    let mut ryang = Ryang::default();
    let mut yang_files = Vec::new();

    match cli.mode {
        Mode::Directory => {
            for path in &cli.paths {
                let dir_path = Path::new(path);
                if !dir_path.is_dir() {
                    eprintln!("Error: {} is not a directory", path);
                    std::process::exit(1);
                }
                collect_yang_files(dir_path, &mut yang_files)?;
            }
            if yang_files.is_empty() {
                eprintln!("Error: no yang files found in {:?}", cli.paths);
                std::process::exit(1);
            }
        }
        Mode::File => {
            for path in &cli.paths {
                let file_path = Path::new(path);
                if !file_path.is_file()
                    || file_path.extension().and_then(|s| s.to_str()) != Some("yang")
                {
                    eprintln!("Error: unexpected file extension of {}", path);
                    std::process::exit(1);
                }
                yang_files.push(file_path.to_path_buf());
            }
        }
    }

    for yang_file in yang_files {
        let yang_file_path = yang_file.to_string_lossy().to_string();
        let content = fs::read_to_string(&yang_file)
            .with_context(|| format!("Failed to read {}", yang_file.display()))?;
        ryang.parse(&yang_file_path, &content, 0)?;
    }
    let modules = ryang.list();
    println!("Loaded {} YANG modules", modules.len());

    Ok(())
}

fn collect_yang_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_yang_files(&path, files)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("yang") {
            files.push(path);
        }
    }
    Ok(())
}
