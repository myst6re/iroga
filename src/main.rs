mod iro_entry;
mod iro_header;

use std::{
    io::{BufRead, BufReader, Seek, Write},
    path::{Path, PathBuf},
    process,
    result::Result,
};

use clap::{Args, Parser, Subcommand};
use iro_entry::{FileFlags, IroEntry, INDEX_FIXED_BYTE_SIZE};
use iro_header::{IroFlags, IroHeader, IroVersion};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

/// Command line tool to pack a single directory into a single archive in IRO format
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pack a single directory into an IRO archive
    Pack(PackArgs),
}

#[derive(Args)]
struct PackArgs {
    /// Directory to pack
    #[arg()]
    dir: PathBuf,

    /// Output file path (default is the name of the dir to pack)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] ::std::io::Error),
    #[error(transparent)]
    StripPrefix(#[from] ::std::path::StripPrefixError),
    #[error("{0} is not a directory")]
    NotDir(PathBuf),
    #[error("output file path already exists: {0}")]
    OutputPathExists(PathBuf),
    #[error("{0} has invalid unicode")]
    InvalidUnicode(PathBuf),
    #[error("could not find default name from {0}")]
    CannotDetectDefaultName(PathBuf),
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Pack(args) => match pack_archive(args.dir, args.output) {
            Ok(output_filename) => {
                println!(
                    "archive \"{}\" has been created!",
                    output_filename.display()
                );
                process::exit(0);
            }
            Err(err) => {
                let stderr = std::io::stderr();
                writeln!(stderr.lock(), "[iroga error]: {}", err).ok();
                process::exit(1);
            }
        },
    }
}

fn pack_archive(dir_to_pack: PathBuf, output_path: Option<PathBuf>) -> Result<PathBuf, Error> {
    let dir_metadata = std::fs::metadata(&dir_to_pack)?;
    if !dir_metadata.is_dir() {
        return Err(Error::NotDir(dir_to_pack));
    }

    // compute output filepath: either default generated name or given output_path
    let output_path = match output_path {
        Some(path) => path,
        None => {
            let abs_path = std::fs::canonicalize(&dir_to_pack)?;
            let mut filename = abs_path
                .file_name()
                .ok_or(Error::CannotDetectDefaultName(abs_path.clone()))?
                .to_owned();
            filename.push(".iro");
            Path::new(&filename).to_owned()
        }
    };

    // Do not create IRO archive if the output path already points to an existing file
    if std::fs::File::open(&output_path).is_ok() {
        return Err(Error::OutputPathExists(output_path));
    }

    let entries: Vec<DirEntry> = WalkDir::new(&dir_to_pack)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| !e.file_type().is_dir())
        .collect();
    let mut mod_file = std::fs::File::create(&output_path)?;

    // IRO Header
    let iro_header = IroHeader::new(IroVersion::Two, IroFlags::None, 16, entries.len() as u32);
    let iro_header_bytes = Vec::from(iro_header);
    let iro_header_size = iro_header_bytes.len() as u64;
    mod_file.write_all(iro_header_bytes.as_ref())?;

    let mut offset = iro_header_size;
    for entry in &entries {
        let unicode_filepath: Vec<u8> =
            unicode_filepath_bytes(entry.path(), dir_to_pack.as_path())?;
        offset += (unicode_filepath.len() + INDEX_FIXED_BYTE_SIZE) as u64;
    }
    mod_file.seek(std::io::SeekFrom::Start(offset))?;

    let mut iro_entries: Vec<IroEntry> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let file = std::fs::File::open(entry.to_owned().into_path())?;
        let entry_offset = offset;
        let mut reader = BufReader::new(file);
        loop {
            let bytes = reader.fill_buf()?;
            let consumed = bytes.len();
            if consumed == 0 {
                break;
            }
            mod_file.write_all(bytes)?;
            reader.consume(consumed);
            offset += consumed as u64;
        }
        iro_entries.push(IroEntry::new(
            unicode_filepath_bytes(entry.path(), dir_to_pack.as_path())?,
            FileFlags::Uncompressed,
            entry_offset,
            (offset - entry_offset) as u32,
        ));
    }

    // indexing data
    mod_file.seek(std::io::SeekFrom::Start(iro_header_size))?;
    for entry in iro_entries {
        mod_file.write_all(&Vec::from(entry))?;
    }

    Ok(output_path)
}

fn unicode_filepath_bytes(path: &Path, strip_prefix_str: &Path) -> Result<Vec<u8>, Error> {
    Ok(path
        .strip_prefix(strip_prefix_str)?
        .to_str()
        .ok_or(Error::InvalidUnicode(path.to_owned()))?
        .replace('/', "\\")
        .encode_utf16()
        .flat_map(|ch| ch.to_le_bytes())
        .collect())
}
