// based on https://docs.rs/bio/0.32.0/bio/io/fastq/index.html#read-and-write
use bio::io::fastq;
use clap::AppSettings::DeriveDisplayOrder;
use clap::Parser;
use minimap2::*;
use rayon::prelude::*;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;

// The arguments end up in the Cli struct
#[derive(Parser, Debug)]
#[structopt(global_settings=&[DeriveDisplayOrder])]
#[clap(author, version, about="Filtering and trimming of fastq files. Reads on stdin and writes to stdout.", long_about = None)]
struct Cli {
    /// Sets a minimum Phred average quality score
    #[clap(short = 'q', long = "quality", value_parser, default_value_t = 0.0)]
    minqual: f64,

    /// Sets a minimum read length
    #[clap(short = 'l', long, value_parser, default_value_t = 1)]
    minlength: usize,

    /// Sets a maximum read length
    // Default is largest i32. Better would be to explicitly use Inf, but couldn't figure it out.
    #[clap(long, value_parser, default_value_t = 2147483647)]
    maxlength: usize,

    /// Trim N nucleotides from the start of a read
    #[clap(long, value_parser, default_value_t = 0)]
    headcrop: usize,

    /// Trim N nucleotides from the end of a read
    #[clap(long, value_parser, default_value_t = 0)]
    tailcrop: usize,

    /// Use N parallel threads
    #[clap(short, long, value_parser, default_value_t = 4)]
    threads: usize,

    /// Filter contaminants against a fasta
    #[clap(short, long, validator = is_file)]
    contam: Option<String>,
}

fn is_file(pathname: &str) -> Result<(), String> {
    let path = PathBuf::from(pathname);
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("Input file {} is invalid", path.display()))
    }
}

fn main() {
    let args = Cli::parse();
    filter(&mut io::stdin(), args);
}

/// This function filters fastq on stdin based on quality, maxlength and minlength
/// and applies trimming before writting to stdout
fn filter(input: &mut impl Read, args: Cli) {
    match args.contam {
        Some(ref fas) => {
            let aligner = setup_contamination_filter(fas);
            fastq::Reader::new(input)
                .records()
                .into_iter()
                .for_each(|record| {
                    let record = record.unwrap();
                    if !record.is_empty() {
                        let read_len = record.seq().len();
                        // If a read is shorter than what is to be cropped the read is dropped entirely (filtered out)
                        if args.headcrop + args.tailcrop < read_len {
                            let average_quality = ave_qual(record.qual());
                            if average_quality >= args.minqual
                                && read_len >= args.minlength
                                && read_len <= args.maxlength
                                && !is_contamination(&record.seq(), &aligner)
                            {
                                // Check if a description attribute is present, taken from the bio-rust code to format fastq
                                let header = match record.desc() {
                                    Some(d) => format!("{} {}", record.id(), d),
                                    None => record.id().to_owned(),
                                };
                                // Print out the records passing the filters, applying trimming on seq and qual
                                // Could consider to use unsafe `from_utf8_unchecked`
                                println!(
                                    "@{}\n{}\n+\n{}",
                                    header,
                                    std::str::from_utf8(
                                        &record.seq()[args.headcrop..read_len - args.tailcrop]
                                    )
                                    .unwrap(),
                                    std::str::from_utf8(
                                        &record.qual()[args.headcrop..read_len - args.tailcrop]
                                    )
                                    .unwrap()
                                );
                            }
                        }
                    }
                });
        }

        None => {
            rayon::ThreadPoolBuilder::new()
                .num_threads(args.threads)
                .build()
                .unwrap();
            fastq::Reader::new(io::stdin())
                .records()
                .into_iter()
                .par_bridge()
                .for_each(|record| {
                    let record = record.unwrap();
                    if !record.is_empty() {
                        let read_len = record.seq().len();
                        // If a read is shorter than what is to be cropped the read is dropped entirely (filtered out)
                        if args.headcrop + args.tailcrop < read_len {
                            let average_quality = ave_qual(record.qual());
                            if average_quality >= args.minqual
                                && read_len >= args.minlength
                                && read_len <= args.maxlength
                            {
                                // Check if a description attribute is present, taken from the bio-rust code to format fastq
                                let header = match record.desc() {
                                    Some(d) => format!("{} {}", record.id(), d),
                                    None => record.id().to_owned(),
                                };
                                // Print out the records passing the filters, applying trimming on seq and qual
                                // Could consider to use unsafe `from_utf8_unchecked`
                                println!(
                                    "@{}\n{}\n+\n{}",
                                    header,
                                    std::str::from_utf8(
                                        &record.seq()[args.headcrop..read_len - args.tailcrop]
                                    )
                                    .unwrap(),
                                    std::str::from_utf8(
                                        &record.qual()[args.headcrop..read_len - args.tailcrop]
                                    )
                                    .unwrap()
                                );
                            }
                        }
                    }
                });
        }
    }
}

/// This function calculates the average quality of a read, and does this correctly
/// First the Phred scores are converted to probabilities (10^(q)/-10) and summed
/// and then divided by the number of bases/scores and converted to Phred again -10*log10(average)
fn ave_qual(quals: &[u8]) -> f64 {
    let probability_sum = quals
        .iter()
        .map(|q| 10_f64.powf((*q as f64) / -10.0))
        .sum::<f64>();
    (probability_sum / quals.len() as f64).log10() * -10.0
}

fn setup_contamination_filter(contam_fasta: &str) -> Aligner {
    Aligner {
        threads: 8,
        ..map_ont()
    }
    .with_index(contam_fasta, None)
    .expect("Unable to build index")
}

// Checks if a sequence is a contaminant, and returns false if so
fn is_contamination(readseq: &&[u8], contam: &Aligner) -> bool {
    let alignment = contam
        .map(readseq, false, false, None, None)
        .expect("Unable to align");
    alignment[0].target_name.is_some()
}

#[test]
fn test_ave_qual() {
    extern crate approx;
    assert_eq!(ave_qual(&[10]), 10.0);
    assert!(approx::abs_diff_eq!(
        ave_qual(&[10, 11, 12]),
        10.923583702678473
    ));
}

#[test]
fn test_filter() {
    filter(
        &mut File::open("test-data/test.fastq").unwrap(),
        Cli {
            minlength: 100,
            maxlength: 100000,
            minqual: 5.0,
            headcrop: 10,
            tailcrop: 10,
            threads: 2,
            contam: None,
        },
    );
}
// FEATURES TO ADD
// Write test for ave_qual
// write integration tests
// package
