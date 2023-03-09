use clap::{App, Arg};
use format_num::format_num;
use std::io::prelude::*;
use std::process::Command;
use std::str;
use std::time::Instant;
use std::usize;
use std::{fs::File, io::BufReader};
mod predictor;
mod weighted_bool;
use predictor::*;

fn read_event<T>(reader: &mut BufReader<T>) -> Option<(usize, bool, usize)>
where
    T: std::io::Read,
{
    let mut event_buf: [u8; 8] = [0; 8];
    if let Ok(bytes_read) = reader.read(&mut event_buf) {
        if bytes_read == 8 {
            let event = i64::from_le_bytes(event_buf);
            let addr: usize = ((event << 16) >> 16) as usize;
            let was_taken: bool = event < 0;
            let delta: usize = (event as usize >> 48) & 0x7FFF;

            return Some((addr, was_taken, delta));
        }
    }

    None
}

fn report(
    predictors: Vec<Box<dyn Predictor>>,
    elapsed: std::time::Duration,
    count: usize,
    instret: usize,
) -> Result<(), std::io::Error> {
    println!(
        "Processed {} branch events ({} predictions) in {:.2} s = {:.3} Mpredictions/s",
        format_num!(",.0", count as f64),
        format_num!(",.0", (count * predictors.capacity()) as f64),
        elapsed.as_secs_f64(),
        count as f64 * predictors.capacity() as f64 / (1000000.0 * elapsed.as_secs_f64())
    );

    let mut results: Vec<(String, Vec<usize>, usize, usize)> =
        predictors.iter().map(|p| p.report()).collect();

    results.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

    {
        let mut data = File::create("bp.dat")?;

        for (alg, config, size, misses) in results {
            let miss_rate = misses as f64 / count as f64;
            let mpki = 1000.0 * misses as f64 / instret as f64;
            let hit_rate = 100.0 - 100.0 * miss_rate;
            let kb = size as f64 / 8192.0;

            println!(
                "{:5.1} mpki ({:4.1}%) {:6.1} KiB {} {:?}",
                mpki, hit_rate, kb, alg, config
            );

            writeln!(&mut data, "{}\t{}", size as f64 / 8192.0, mpki)?;
        }
    }

    let output = Command::new("gnuplot")
        .args(["plot.gp"])
        .output()
        .expect("failed to launch gnuplot")
        .stdout;

    /* GNUplot it */
    print!("{}", str::from_utf8(&output).expect("Bad UTF-8"));

    Ok(())
}

// XXX It would be nice to turn this into an iterator
fn run(mut predictors: Vec<Box<dyn Predictor>>, file_name: &str) -> Result<(), std::io::Error> {
    let file = File::open(file_name)?;
    let mut reader = BufReader::new(file);
    let mut header = [0; 1024];
    reader.read_exact(&mut header)?;

    /*
        let queue = Arc::new(MsQueue::new());
        let handles: Vec<_> = (1..8)
            .map(|_| {
                let t_queue = queue.clone();
                thread::spawn(move || {
                    while let Some(i) = t_queue.try_pop() {

                    }
                })
            })
            .collect();
    */

    if false {
        match str::from_utf8(&header) {
            Ok(v) => println!("Header: {}", v),
            Err(e) => panic!("Invalid UTF-8 sequence: {}", e),
        };
    }

    let start = Instant::now();

    let (mut count, mut instret) = (0, 0);
    while let Some((addr, was_taken, delta)) = read_event(&mut reader) {
        instret += delta + 1;

        for p in predictors.iter_mut() {
            p.predict_and_update(addr, was_taken);
        }

        count += 1;
    }

    let elapsed = start.elapsed();

    report(predictors, elapsed, count, instret)
}

fn gen_predictors() -> Vec<Box<dyn Predictor>> {
    let mut predictors: Vec<Box<dyn Predictor>> = if false {
        vec![Box::new(NoneTakenBp::new()), Box::new(LocalBp::new(14))]
    } else {
        vec![]
    };

    if false {
        for s in 12..=18 {
            predictors.push(Box::new(GshareBp::new(s)));
        }
        for s in 10..=17 {
            predictors.push(Box::new(BimodalBp::new(s)));
        }
    }

    if true {
        for d in 0..5 {
            let s = 13;
            predictors.push(Box::new(Yags1Bp::new(s, s - d, 6)));
            predictors.push(Box::new(Yags2Bp::new(s, s - d, 6)));
            predictors.push(Box::new(Yags3Bp::new(s, s - d, 6)));
            predictors.push(Box::new(Yags4Bp::new(s, s - d, 6)));
        }
    }

    //    predictors.push(Box::new(Yags5Bp::new(22, 22, 22)));

    // Limit test
    // predictors.push(Box::new(Yags1Bp::new(22, 40)));
    predictors
}

fn main() {
    let matches = App::new("Bp")
        .version("1.0")
        .author("Tommy Thorn <tommy.thorn@gmail.com>")
        .about("Exercizes Branch Predictor Algorithms")
        .arg(
            Arg::with_name("INPUT")
                .help("Sets the input file to use")
                .required(true)
                .index(1),
        )
        .get_matches();

    let input = matches.value_of("INPUT").unwrap();
    run(gen_predictors(), input).expect("failed to read file");
}
