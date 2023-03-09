use crate::weighted_bool::*;

pub trait Predictor {
    // XXX Make predict_and_update process a batch of branch events
    fn predict_and_update(&mut self, addr: usize, was_taken: bool);

    fn report(&self) -> (String, Vec<usize>, usize, usize);
}

pub struct NoneTakenBp {
    misses: usize,
}

impl NoneTakenBp {
    pub fn new() -> NoneTakenBp {
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

pub struct LocalBp {
    addr_bits: usize,
    pht: Vec<TwoBitCounter>,
    addr_mask: usize,
    misses: usize,
}

impl LocalBp {
    pub fn new(addr_bits: usize) -> LocalBp {
        let pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
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
        let predicted: bool = self.pht[index].value();
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

pub struct GshareBp {
    addr_bits: usize,
    history: usize,
    pht: Vec<TwoBitCounter>,
    addr_mask: usize,
    misses: usize,
}

impl GshareBp {
    pub fn new(addr_bits: usize) -> GshareBp {
        GshareBp {
            addr_bits,
            history: 0,
            pht: vec![TwoBitCounter::new(true); 1 << addr_bits],
            addr_mask: (1 << addr_bits) - 1,
            misses: 0,
        }
    }
}

impl Predictor for GshareBp {
    fn predict_and_update(&mut self, addr: usize, was_taken: bool) {
        let index = ((addr >> 1) ^ self.history) & self.addr_mask;
        let predicted: bool = self.pht[index].value();
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

pub struct BimodalBp {
    addr_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitCounter>,
    direction_pht_nt: Vec<TwoBitCounter>,
    direction_pht_t: Vec<TwoBitCounter>,
    addr_mask: usize,
    misses: usize,
}

impl BimodalBp {
    pub fn new(addr_bits: usize) -> BimodalBp {
        let choice_pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht_nt = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht_t = vec![TwoBitCounter::new(true); 1 << addr_bits];
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
        let choice_index = (addr >> 1) & self.addr_mask;
        let direction_index = ((addr >> 1) ^ self.history) & self.addr_mask;

        let choice = self.choice_pht[choice_index].value();

        let predicted;

        if choice {
            predicted = self.direction_pht_t[direction_index].value();
            self.direction_pht_t[direction_index].update(was_taken);
        } else {
            predicted = self.direction_pht_nt[direction_index].value();
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
pub struct Yags1Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitCounter>,
    direction_pht: Vec<TwoBitCounter>,
    direction_tag: Vec<usize>,
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags1Bp {
    pub fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags1Bp {
        let choice_pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht = vec![TwoBitCounter::new(true); 1 << dir_bits];
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
            self.direction_pht[hash_index].value()
        } else {
            self.choice_pht[addr_index].value()
        };

        // Update
        if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].update(was_taken);
        } else {
            // The choice is updated on misses
            self.choice_pht[addr_index].update(was_taken);
            if self.choice_pht[addr_index].value() != was_taken {
                self.direction_tag[hash_index] = hash_tag;
                self.direction_pht[hash_index] = TwoBitCounter::new(was_taken);
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
pub struct Yags2Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitCounter>,
    direction_pht: Vec<TwoBitCounter>,
    direction_tag: Vec<usize>,
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags2Bp {
    pub fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags2Bp {
        let choice_pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht = vec![TwoBitCounter::new(true); 1 << dir_bits];
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
            self.direction_pht[hash_index].value()
        } else {
            self.choice_pht[addr_index].value()
        };

        // Update
        if self.direction_tag[hash_index] == hash_tag {
            self.direction_pht[hash_index].update(was_taken);
        } else {
            // The choice is updated on misses
            self.choice_pht[addr_index].update(was_taken);
            if self.choice_pht[addr_index].value() != was_taken {
                self.direction_tag[hash_index] = hash_tag;
                self.direction_pht[hash_index] = TwoBitCounter::new(was_taken);
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
pub struct Yags3Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitCounter>,
    direction_pht: [Vec<TwoBitCounter>; 2],
    direction_tag: [Vec<usize>; 2],
    direction_u: [Vec<bool>; 2],
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags3Bp {
    pub fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags3Bp {
        let choice_pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht = [
            vec![TwoBitCounter::new(true); 1 << dir_bits],
            vec![TwoBitCounter::new(true); 1 << dir_bits],
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
            self.direction_pht[0][hash_index].value()
        } else if self.direction_tag[1][hash_index] == hash_tag {
            used = Some(1);
            self.direction_pht[1][hash_index].value()
        } else {
            used = None;
            self.choice_pht[addr_index].value()
        };

        // Update
        match used {
            Some(n) => {
                self.direction_pht[n][hash_index].update(was_taken);
                self.direction_u[n][hash_index] =
                    self.direction_pht[n][hash_index].value() == was_taken;
            }
            None => {
                // The choice is updated on misses
                self.choice_pht[addr_index].update(was_taken);

                // NB: this is key no not waste an entry needlessly
                if self.choice_pht[addr_index].value() != was_taken {
                    if !self.direction_u[0][hash_index] {
                        self.direction_tag[0][hash_index] = hash_tag;
                        self.direction_pht[0][hash_index] = TwoBitCounter::new(was_taken);
                    } else if !self.direction_u[1][hash_index] {
                        self.direction_tag[1][hash_index] = hash_tag;
                        self.direction_pht[1][hash_index] = TwoBitCounter::new(was_taken);
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
pub struct Yags4Bp {
    addr_bits: usize,
    dir_bits: usize,
    tag_bits: usize,
    history: usize,
    choice_pht: Vec<TwoBitCounter>,
    direction_pht: [Vec<TwoBitCounter>; 2],
    direction_tag: [Vec<usize>; 2],
    direction_u: [Vec<bool>; 2],
    addr_mask: usize,
    dir_mask: usize,
    tag_mask: usize,
    misses: usize,
}

impl Yags4Bp {
    pub fn new(addr_bits: usize, dir_bits: usize, tag_bits: usize) -> Yags4Bp {
        let dir_entries = 1 << dir_bits;
        let choice_pht = vec![TwoBitCounter::new(true); 1 << addr_bits];
        let direction_pht = [
            vec![TwoBitCounter::new(true); dir_entries],
            vec![TwoBitCounter::new(true); dir_entries],
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
            self.direction_pht[0][hash_index].value()
        } else if self.direction_tag[1][hash_index] == hash_tag {
            used = Some(1);
            self.direction_pht[1][hash_index].value()
        } else {
            used = None;
            self.choice_pht[addr_index].value()
        };

        // Update
        match used {
            Some(n) => {
                self.direction_pht[n][hash_index].update(was_taken);
                self.direction_u[n][hash_index] =
                    self.direction_pht[n][hash_index].value() == was_taken;
            }
            None => {
                // The choice is updated on misses
                self.choice_pht[addr_index].update(was_taken);

                // NB: this is key no not waste an entry needlessly
                if self.choice_pht[addr_index].value() != was_taken {
                    if !self.direction_u[0][hash_index] {
                        self.direction_tag[0][hash_index] = hash_tag;
                        self.direction_pht[0][hash_index] = TwoBitCounter::new(was_taken);
                    } else if !self.direction_u[1][hash_index] {
                        self.direction_tag[1][hash_index] = hash_tag;
                        self.direction_pht[1][hash_index] = TwoBitCounter::new(was_taken);
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
