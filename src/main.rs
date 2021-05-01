extern crate clap;
use clap::{App, Arg};
use format_num::format_num;
use std::io::prelude::*;
use std::process::Command;
use std::str;
use std::time::Instant;
use std::usize;
use std::{fs::File, io::BufReader};

//use crossbeam::sync::MsQueue;

// TODO:
// - separate prediction and update, enabling modelling delayed updates

// NB. Not using enums in order to use a bit encoding trick
type TwoBitWeight = i8;
const _STRONGLY_NOT_TAKEN: TwoBitWeight = 0;
const WEAKLY_NOT_TAKEN: TwoBitWeight = 1;
const WEAKLY_TAKEN: TwoBitWeight = 2;
const _STRONGLY_TAKEN: TwoBitWeight = 3;

trait SaturatingBoolCounters {
    fn update(&mut self, taken: bool) -> &mut Self;
    fn weakly_taken() -> Self;
    fn weakly_not_taken() -> Self;
    fn to_bool(&self) -> bool;
}

const SCALE: usize = 5;

fn from_bool(b: bool) -> TwoBitWeight {
    if b {
        WEAKLY_TAKEN << SCALE
    } else {
        WEAKLY_NOT_TAKEN << SCALE
    }
}

impl SaturatingBoolCounters for TwoBitWeight {
    fn update(&mut self, taken: bool) -> &mut Self {
        /* Conceptually

        if taken {
            if *self != 3 { *self += 1 }
        } else {
            if *self != 0 { *self -= 1 }
        }

        However, using the signbit to capture overflow and expand that
        to mux in the old value let's us do this branch-free and faster:

        let overflow_mask = (new << 29) >> 31;
        assert_eq!(overflow_mask, if new & 3 == new { 0 } else { -1 });
         *self = (*self as i32 & overflow_mask | new & (!overflow_mask)) as i8;

        To save a shift, we use the prescaled representation of the values.
        */

        let new = *self + ((taken as i8) * (2 << SCALE) - (1 << SCALE));
        let overflow_mask = (new << (5 - SCALE)) >> 7;
        *self = *self & overflow_mask | new & !overflow_mask;

        self
    }

    fn weakly_taken() -> Self {
        WEAKLY_TAKEN << SCALE
    }

    fn weakly_not_taken() -> Self {
        WEAKLY_NOT_TAKEN << SCALE
    }

    fn to_bool(&self) -> bool {
        Self::weakly_taken() <= *self
    }
}

#[test]
fn idempodence() {
    // Level 0 sanity - idempodence
    assert_eq!(from_bool(false).to_bool(), false);
    assert_eq!(from_bool(true).to_bool(), true);
}

#[test]
fn strengthening() {
    // Level 1 sanity - strengthening
    assert_eq!(from_bool(false).update(false).to_bool(), false);
    assert_eq!(from_bool(true).update(true).to_bool(), true);
}

#[test]
fn weak_update() {
    // Level 2 sanity - weak + change
    assert_eq!(from_bool(false).update(true).to_bool(), true);
    assert_eq!(from_bool(true).update(false).to_bool(), false);
}

#[test]
fn strong_update() {
    // Level 3 sanity - strong + change
    assert_eq!(from_bool(false).update(false).update(true).to_bool(), false);
    assert_eq!(from_bool(true).update(true).update(false).to_bool(), true);

    // Level 4 sanity - strong + change*2
    assert_eq!(
        from_bool(false)
            .update(false)
            .update(true)
            .update(true)
            .to_bool(),
        true
    );
    assert_eq!(
        from_bool(true)
            .update(true)
            .update(false)
            .update(false)
            .to_bool(),
        false
    );
}

trait Predictor {
    // XXX Make predict_and_update process a batch of branch events
    fn predict_and_update(&mut self, addr: usize, was_taken: bool);

    fn report(&self) -> (String, Vec<usize>, usize, usize);
}

struct NoneTakenBp {
    misses: usize,
}

impl NoneTakenBp {
    fn new() -> NoneTakenBp {
        NoneTakenBp { misses: 0 }
    }
}

impl Predictor for NoneTakenBp {
    fn predict_and_update(&mut self, _addr: usize, was_taken: bool) {
        self.misses += was_taken as usize;
    }
    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        ("NoneTaken".to_string(), vec![], 0, self.misses)
    }
}

struct LocalBp {
    addr_bits: usize,
    pht: Vec<TwoBitWeight>,
    addr_mask: usize,
    misses: usize,
}

impl LocalBp {
    fn new(addr_bits: usize) -> LocalBp {
        let pht = vec![TwoBitWeight::weakly_taken(); 1 << addr_bits];
        LocalBp {
            addr_bits,
            pht,
            addr_mask: (1 << addr_bits) - 1,
            misses: 0,
        }
    }
}

impl Predictor for LocalBp {
    fn predict_and_update(&mut self, addr: usize, was_taken: bool) {
        let index = (addr >> 1) & self.addr_mask;
        let predicted: bool = self.pht[index].to_bool();
        self.pht[index].update(was_taken);
        self.misses += (predicted != was_taken) as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "Two-level".to_string(),
            vec![self.addr_bits],
            (1 << self.addr_bits) * 2,
            self.misses,
        )
    }
}

struct GshareBp {
    addr_bits: usize,
    history: usize,
    pht: Vec<TwoBitWeight>,
    addr_mask: usize,
    misses: usize,
}

impl GshareBp {
    fn new(addr_bits: usize) -> GshareBp {
        GshareBp {
            addr_bits,
            history: 0,
            pht: vec![TwoBitWeight::weakly_taken(); 1 << addr_bits],
            addr_mask: (1 << addr_bits) - 1,
            misses: 0,
        }
    }
}

impl Predictor for GshareBp {
    fn predict_and_update(&mut self, addr: usize, was_taken: bool) {
        let index = (((addr >> 1) ^ self.history) & self.addr_mask) as usize;
        let predicted: bool = self.pht[index].to_bool();
        self.pht[index].update(was_taken);
        self.misses += (predicted != was_taken) as usize;
        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "Gshare".to_string(),
            vec![self.addr_bits],
            (1 << self.addr_bits) * 2,
            self.misses,
        )
    }
}

struct BimodalBp {
    addr_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitWeight>,
    direction_pht_nt: Vec<TwoBitWeight>,
    direction_pht_t: Vec<TwoBitWeight>,
    addr_mask: usize,
    misses: usize,
}

impl BimodalBp {
    fn new(addr_bits: usize) -> BimodalBp {
        let choice_pht = vec![TwoBitWeight::weakly_taken(); 1 << addr_bits];
        let direction_pht_nt = vec![TwoBitWeight::weakly_taken(); 1 << addr_bits];
        let direction_pht_t = vec![TwoBitWeight::weakly_taken(); 1 << addr_bits];
        BimodalBp {
            addr_bits,
            history: 0,
            choice_pht,
            direction_pht_nt,
            direction_pht_t,
            addr_mask: (1 << addr_bits) - 1,
            misses: 0,
        }
    }
}

impl Predictor for BimodalBp {
    fn predict_and_update(&mut self, addr: usize, was_taken: bool) {
        let choice_index = ((addr >> 1) & self.addr_mask) as usize;
        let direction_index = (((addr >> 1) ^ self.history) & self.addr_mask) as usize;

        let choice = self.choice_pht[choice_index].to_bool();

        let predicted;

        if choice {
            predicted = self.direction_pht_t[direction_index].to_bool();
            self.direction_pht_t[direction_index].update(was_taken);
        } else {
            predicted = self.direction_pht_nt[direction_index].to_bool();
            self.direction_pht_nt[direction_index].update(was_taken);
        };

        /* "The choice PHT is normally updated too, but not if it
         * gives a prediction contradicting the branch outcome and the
         * direction PHT chosen gives the correct prediction."
         *
         * That is, it's updated if we mispredicted or it disagreed
         * with the actual direction */

        if predicted != was_taken || choice != was_taken {
            self.choice_pht[choice_index].update(was_taken);
        }

        if predicted != was_taken {
            self.misses += 1;
        }

        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "Bimodal".to_string(),
            vec![self.addr_bits],
            (self.choice_pht.capacity()
                + self.direction_pht_t.capacity()
                + self.direction_pht_nt.capacity())
                * 2,
            self.misses,
        )
    }
}

// YAGS1 = YAGS with a single direction table
struct Yags1Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitWeight>,
    direction_pht: Vec<TwoBitWeight>,
    direction_tag: Vec<usize>,
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags1Bp {
    fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags1Bp {
        let choice_pht = vec![from_bool(true); 1 << addr_bits];
        let direction_pht = vec![from_bool(true); 1 << dir_bits];
        let direction_tag = vec![0; 1 << dir_bits];
        let tag_mask = (1 << tag_bits) - 1;
        Yags1Bp {
            addr_bits,
            dir_bits,
            tag_bits,
            history: 0,
            choice_pht,
            direction_pht,
            direction_tag,
            addr_mask: (1 << addr_bits) - 1,
            dir_mask: (1 << dir_bits) - 1,
            tag_mask,
            misses: 0,
        }
    }
}

impl Predictor for Yags1Bp {
    fn predict_and_update(&mut self, mut addr: usize, was_taken: bool) {
        // First drop the constant zero LSB
        addr >>= 1;

        // Split the address into index and tag
        let addr_index = (addr >> 1) & self.addr_mask;
        let hash_index = (addr ^ self.history) & self.dir_mask;
        let hash_tag = addr & self.tag_mask;

        // Access
        let predicted = if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].to_bool()
        } else {
            self.choice_pht[addr_index].to_bool()
        };

        // Update
        if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].update(was_taken);
        } else {
            // The choice is updated on misses
            self.choice_pht[addr_index].update(was_taken);
            if self.choice_pht[addr_index].to_bool() != was_taken {
                self.direction_tag[hash_index] = hash_tag;
                self.direction_pht[hash_index] = from_bool(was_taken);
            }
        }

        if predicted != was_taken {
            self.misses += 1;
        }

        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "YAGS1".to_string(),
            vec![self.addr_bits, self.dir_bits, self.tag_bits],
            self.choice_pht.capacity() * 2 + self.direction_pht.capacity() * (2 + self.tag_bits),
            self.misses,
        )
    }
}

/* YAGS2 = YAGS1 + history hashed index  */
struct Yags2Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitWeight>,
    direction_pht: Vec<TwoBitWeight>,
    direction_tag: Vec<usize>,
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags2Bp {
    fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags2Bp {
        let choice_pht = vec![from_bool(true); 1 << addr_bits];
        let direction_pht = vec![from_bool(true); 1 << dir_bits];
        let direction_tag = vec![0; 1 << dir_bits];
        let tag_mask = (1 << tag_bits) - 1;
        Yags2Bp {
            addr_bits,
            dir_bits,
            tag_bits,
            history: 0,
            choice_pht,
            direction_pht,
            direction_tag,
            addr_mask: (1 << addr_bits) - 1,
            dir_mask: (1 << dir_bits) - 1,
            tag_mask,
            misses: 0,
        }
    }
}

impl Predictor for Yags2Bp {
    fn predict_and_update(&mut self, mut addr: usize, was_taken: bool) {
        // First drop the constant zero LSB
        addr >>= 1;

        /*
         * For the history bits in the tag, use a different fold than
         * what you use in the index something like
         *
         *   {address_bits[1:4], address_bits[5:8] ^ history_bits}
         *
         * for the index having folding is good if you decide you want
         * larger history than your index bit Us
         *
         *   ((addr >> 1) & 0xf) << 4) | (((addr >> 4) ^ hist) & 0xf)
         *
         * when you add history to the tag, keep some bits from being
         * xored probably easier to say:
         *
         *   addr ^ ((hist << 4) & 0xff)
         *
         */

        let addr_index = (addr >> 1) & self.addr_mask;

        // This was very poor
        //        let hash_index = (((addr >> 1) & 15) * 16 + ((addr >> 5) ^ history) & 15) & self.addr_mask;
        //        let hash_tag   = (addr ^ (history << 4)) & self.tag_mask;

        //      let hash_index = (((addr >> 1) & 15) * 16 + ((addr >> 5) ^ self.history) & 15) & self.addr_mask;

        let hash_index = (addr ^ self.history) & self.dir_mask;

        // {address_bits[1:4], address_bits[5:8] ^ history_bits}
        let hash_tag = ((addr & 30) << 4 | (addr >> 5 ^ self.history) & 15) & self.tag_mask;

        // Access
        let predicted = if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].to_bool()
        } else {
            self.choice_pht[addr_index].to_bool()
        };

        // Update
        if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].update(was_taken);
        } else {
            // The choice is updated on misses
            self.choice_pht[addr_index].update(was_taken);
            if self.choice_pht[addr_index].to_bool() != was_taken {
                self.direction_tag[hash_index] = hash_tag;
                self.direction_pht[hash_index] = from_bool(was_taken);
            }
        }

        if predicted != was_taken {
            self.misses += 1;
        }

        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "YAGS2".to_string(),
            vec![self.addr_bits, self.dir_bits, self.tag_bits],
            self.choice_pht.capacity() * 2 + self.direction_pht.capacity() * (2 + self.tag_bits),
            self.misses,
        )
    }
}

/* YAGS3 = YAGS1 + u-bits + 2-way associative directions */
struct Yags3Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitWeight>,
    direction_pht: [Vec<TwoBitWeight>; 2],
    direction_tag: [Vec<usize>; 2],
    direction_u: [Vec<bool>; 2],
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags3Bp {
    fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags3Bp {
        let choice_pht = vec![from_bool(true); 1 << addr_bits];
        let direction_pht = [
            vec![from_bool(true); 1 << dir_bits],
            vec![from_bool(true); 1 << dir_bits],
        ];
        let direction_tag = [vec![0; 1 << dir_bits], vec![0; 1 << dir_bits]];
        let direction_u = [vec![false; 1 << dir_bits], vec![false; 1 << dir_bits]];
        let tag_mask = (1 << tag_bits) - 1;
        Yags3Bp {
            addr_bits,
            dir_bits,
            tag_bits,
            history: 0,
            choice_pht,
            direction_pht,
            direction_tag,
            direction_u,
            addr_mask: (1 << addr_bits) - 1,
            dir_mask: (1 << dir_bits) - 1,
            tag_mask,
            misses: 0,
        }
    }
}

impl Predictor for Yags3Bp {
    fn predict_and_update(&mut self, mut addr: usize, was_taken: bool) {
        // First drop the constant zero LSB
        addr >>= 1;

        let addr_index = (addr >> 1) & self.addr_mask;
        let hash_index = (addr ^ self.history) & self.dir_mask;
        let hash_tag = addr & self.tag_mask;

        // Access
        let used;
        let predicted = if self.direction_tag[0][hash_index] == hash_tag {
            used = Some(0);
            self.direction_pht[0][hash_index].to_bool()
        } else if self.direction_tag[1][hash_index] == hash_tag {
            used = Some(1);
            self.direction_pht[1][hash_index].to_bool()
        } else {
            used = None;
            self.choice_pht[addr_index].to_bool()
        };

        // Update
        match used {
            Some(n) => {
                self.direction_pht[n][hash_index].update(was_taken);
                self.direction_u[n][hash_index] =
                    self.direction_pht[n][hash_index].to_bool() == was_taken;
            }
            None => {
                // The choice is updated on misses
                self.choice_pht[addr_index].update(was_taken);

                // NB: this is key no not waste an entry needlessly
                if self.choice_pht[addr_index].to_bool() != was_taken {
                    if !self.direction_u[0][hash_index] {
                        self.direction_tag[0][hash_index] = hash_tag;
                        self.direction_pht[0][hash_index] = from_bool(was_taken);
                    } else if !self.direction_u[1][hash_index] {
                        self.direction_tag[1][hash_index] = hash_tag;
                        self.direction_pht[1][hash_index] = from_bool(was_taken);
                    } else {
                        self.direction_u[0][hash_index] = false;
                        self.direction_u[1][hash_index] = false;
                    }
                }
            }
        }

        if predicted != was_taken {
            self.misses += 1;
        }

        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "YAGS3".to_string(),
            vec![self.addr_bits, self.dir_bits, self.tag_bits],
            self.choice_pht.capacity() * 2
                + self.direction_pht[0].capacity() * 2 * (3 + self.tag_bits),
            self.misses,
        )
    }
}

/* YAGS4 = YAGS2 + YAGS3 */
struct Yags4Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitWeight>,
    direction_pht: [Vec<TwoBitWeight>; 2],
    direction_tag: [Vec<usize>; 2],
    direction_u: [Vec<bool>; 2],
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags4Bp {
    fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags4Bp {
        let dir_entries = 1 << dir_bits;
        let choice_pht = vec![from_bool(true); 1 << addr_bits];
        let direction_pht = [
            vec![from_bool(true); dir_entries],
            vec![from_bool(true); dir_entries],
        ];
        let direction_tag = [vec![0; dir_entries], vec![0; dir_entries]];
        let direction_u = [vec![false; dir_entries], vec![false; dir_entries]];
        let tag_mask = (1 << tag_bits) - 1;
        Yags4Bp {
            addr_bits,
            dir_bits,
            tag_bits,
            history: 0,
            choice_pht,
            direction_pht,
            direction_tag,
            direction_u,
            addr_mask: (1 << addr_bits) - 1,
            dir_mask: (dir_entries) - 1,
            tag_mask,
            misses: 0,
        }
    }
}

impl Predictor for Yags4Bp {
    fn predict_and_update(&mut self, mut addr: usize, was_taken: bool) {
        // First drop the constant zero LSB
        addr >>= 1;

        let addr_index = (addr >> 1) & self.addr_mask;
        let hash_index = ((addr >> 1) ^ self.history) & self.dir_mask;
        let hash_tag = ((addr & 30) << 4 | (addr >> 5 ^ self.history) & 15) & self.tag_mask;

        // Access
        let used;
        let predicted = if self.direction_tag[0][hash_index] == hash_tag {
            used = Some(0);
            self.direction_pht[0][hash_index].to_bool()
        } else if self.direction_tag[1][hash_index] == hash_tag {
            used = Some(1);
            self.direction_pht[1][hash_index].to_bool()
        } else {
            used = None;
            self.choice_pht[addr_index].to_bool()
        };

        // Update
        match used {
            Some(n) => {
                self.direction_pht[n][hash_index].update(was_taken);
                self.direction_u[n][hash_index] =
                    self.direction_pht[n][hash_index].to_bool() == was_taken;
            }
            None => {
                // The choice is updated on misses
                self.choice_pht[addr_index].update(was_taken);

                // NB: this is key no not waste an entry needlessly
                if self.choice_pht[addr_index].to_bool() != was_taken {
                    if !self.direction_u[0][hash_index] {
                        self.direction_tag[0][hash_index] = hash_tag;
                        self.direction_pht[0][hash_index] = from_bool(was_taken);
                    } else if !self.direction_u[1][hash_index] {
                        self.direction_tag[1][hash_index] = hash_tag;
                        self.direction_pht[1][hash_index] = from_bool(was_taken);
                    } else {
                        self.direction_u[0][hash_index] = false;
                        self.direction_u[1][hash_index] = false;
                    }
                }
            }
        }

        if predicted != was_taken {
            self.misses += 1;
        }

        self.history = self.history << 1 | was_taken as usize;
    }

    fn report(&self) -> (String, Vec<usize>, usize, usize) {
        (
            "YAGS4".to_string(),
            vec![self.addr_bits, self.dir_bits, self.tag_bits],
            self.choice_pht.capacity() * 2
                + self.direction_pht[0].capacity() * 2 * (3 + self.tag_bits),
            self.misses,
        )
    }
}

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
        .args(&["plot.gp"])
        .output()
        .expect("failed to launch gnuplot")
        .stdout;

    /* GNUplot it */
    print!("{}", str::from_utf8(&output).expect("Bad UTF-8"));

    Ok(())
}

// XXX It would be nice to turn this into an iterator
fn run(mut predictors: Vec<Box<dyn Predictor>>, file_name: &str) -> Result<(), std::io::Error> {
    let file = File::open(&file_name)?;
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
