use csv::WriterBuilder;
use rayon::prelude::*;
use solana_sdk::{
    bs58,
    signature::{Keypair, Signer},
};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const BATCH_SIZE: usize = 1000;

fn main() -> io::Result<()> {
    display_banner();
    let vanity_string = read_vanity_string()?;
    let case_sensitive = read_case_sensitivity()?;
    let wallet_count_target = read_wallet_count_target()?;
    let max_threads = read_thread_count()?;
    let csv_file_path = "vanity_wallets.csv".to_string();

    prepare_csv_file(&csv_file_path)?;

    let found_count = Arc::new(Mutex::new(0u64));
    let wallet_count = Arc::new(Mutex::new(0));

    let (tx, rx) = mpsc::channel();
    let handles = spawn_threads(
        max_threads,
        vanity_string,
        case_sensitive,
        found_count.clone(),
        wallet_count_target,
        wallet_count.clone(),
        tx,
    );

    let writer_handle = start_csv_writer_thread(rx, csv_file_path);

    // Periodically print the count of generated wallets
    let counter_handle = {
        let wallet_count = wallet_count.clone();
        let found_count = found_count.clone();
        thread::spawn(move || {
            while *found_count.lock().unwrap() < wallet_count_target {
                print!(
                    "\rWallets generated: {} | Found: {}/{}",
                    wallet_count.lock().unwrap(),
                    found_count.lock().unwrap(),
                    wallet_count_target
                );
                io::stdout().flush().unwrap();
                thread::sleep(Duration::from_millis(25));
            }
        })
    };

    for handle in handles {
        let _ = handle.join();
    }

    let _ = writer_handle.join();
    let _ = counter_handle.join();
    report_completion(
        &found_count,
        &wallet_count,
        wallet_count_target,
        Instant::now(),
    );

    Ok(())
}

fn display_banner() {
    println!("██╗   ██╗ █████╗ ███╗   ██╗ █████╗ ██████╗ ██████╗ ██╗   ██╗");
    println!("██║   ██║██╔══██╗████╗  ██║██╔══██╗██╔══██╗██╔══██╗╚██╗ ██╔╝");
    println!("██║   ██║███████║██╔██╗ ██║███████║██║  ██║██║  ██║ ╚████╔╝ ");
    println!("╚██╗ ██╔╝██╔══██║██║╚██╗██║██╔══██║██║  ██║██║  ██║  ╚██╔╝  ");
    println!(" ╚████╔╝ ██║  ██║██║ ╚████║██║  ██║██████╔╝██████╔╝   ██║   ");
    println!("  ╚═══╝  ╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝╚═════╝ ╚═════╝    ╚═╝   ");
    println!("==========================================================\n");
}

fn read_vanity_string() -> io::Result<String> {
    println!("Enter a vanity string (1-9 characters): ");
    let mut vanity_string = String::new();
    io::stdin().read_line(&mut vanity_string)?;
    Ok(vanity_string.trim().to_owned())
}

fn read_case_sensitivity() -> io::Result<bool> {
    println!("Should the search be case-sensitive? (yes/no): ");
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}

fn read_thread_count() -> io::Result<usize> {
    println!("Enter the number of threads to use: ");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    input
        .trim()
        .parse::<usize>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn read_wallet_count_target() -> io::Result<u64> {
    println!("Enter the number of wallets to find: ");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    input
        .trim()
        .parse::<u64>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn prepare_csv_file(path: &str) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "Public Key,Private Key,Note")?;
    Ok(())
}

fn spawn_threads(
    max_threads: usize,
    vanity_string: String,
    case_sensitive: bool,
    found_count: Arc<Mutex<u64>>,
    wallet_count_target: u64,
    wallet_count: Arc<Mutex<u64>>,
    tx: mpsc::Sender<(String, String)>,
) -> Vec<thread::JoinHandle<()>> {
    let vanity_lower = vanity_string.to_lowercase();

    (0..max_threads)
        .map(|_| {
            let vanity_string = vanity_string.clone();
            let vanity_lower = vanity_lower.clone();
            let found_count = Arc::clone(&found_count);
            let wallet_count = Arc::clone(&wallet_count);
            let tx = tx.clone();

            thread::spawn(move || {
                let mut batch_keypairs: Vec<(String, String)> = Vec::with_capacity(BATCH_SIZE);

                while *found_count.lock().unwrap() < wallet_count_target {
                    // Generate keypairs in batches
                    batch_keypairs.clear();
                    (0..BATCH_SIZE).for_each(|_| {
                        let keypair = Keypair::new();
                        let public_key = keypair.pubkey().to_string();
                        let private_key = bs58::encode(keypair.to_bytes()).into_string();
                        batch_keypairs.push((public_key, private_key));
                    });

                    // Process the batch
                    let matches: Vec<(String, String)> = batch_keypairs
                        .iter()
                        .filter(|(public_key, _)| {
                            check_vanity_string(
                                public_key,
                                &vanity_string,
                                &vanity_lower,
                                case_sensitive,
                            )
                        })
                        .cloned()
                        .collect();

                    // Update counts
                    {
                        let mut count = wallet_count.lock().unwrap();
                        *count += BATCH_SIZE as u64;
                    }

                    // Send matches
                    for keypair in matches {
                        tx.send(keypair).unwrap();
                        let mut found = found_count.lock().unwrap();
                        *found += 1;
                        if *found >= wallet_count_target {
                            break;
                        }
                    }
                }
            })
        })
        .collect()
}

fn check_vanity_string(
    public_key: &str,
    vanity_string: &str,
    vanity_lower: &str,
    case_sensitive: bool,
) -> bool {
    if case_sensitive {
        public_key.ends_with(vanity_string)
    } else {
        // Avoid allocating new strings for each comparison
        public_key.to_lowercase().ends_with(vanity_lower)
    }
}

fn start_csv_writer_thread(
    rx: mpsc::Receiver<(String, String)>,
    csv_file_path: String,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let file = File::create(&csv_file_path).unwrap();
        let buf_writer = BufWriter::with_capacity(32 * 1024, file); // Increased buffer size
        let mut wtr = WriterBuilder::new().from_writer(buf_writer);

        let mut batch = Vec::with_capacity(100);
        while let Ok((public_key, private_key)) = rx.recv() {
            batch.push(vec![
                public_key,
                private_key,
                "Generated by Vanity".to_string(),
            ]);

            if batch.len() >= 100 {
                for record in &batch {
                    wtr.write_record(record).unwrap();
                }
                wtr.flush().unwrap();
                batch.clear();
            }
        }

        // Write remaining records
        for record in &batch {
            wtr.write_record(record).unwrap();
        }
        wtr.flush().unwrap();
    })
}

fn report_completion(
    found_count: &Arc<Mutex<u64>>,
    wallet_count: &Arc<Mutex<u64>>,
    wallet_count_target: u64,
    start_time: Instant,
) {
    let found = *found_count.lock().unwrap();
    if found >= wallet_count_target {
        println!("\nFound all {} vanity addresses!", wallet_count_target);
    } else {
        println!(
            "\nFound {} out of {} vanity addresses.",
            found, wallet_count_target
        );
    }
    let count = *wallet_count.lock().unwrap();
    println!("Total wallets generated: {}", count);
    println!("Elapsed time: {:?}", start_time.elapsed());
    println!("Results have been saved to vanity_wallets.csv");
}
