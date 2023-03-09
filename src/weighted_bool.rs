use rand::Rng;

/**
 * Boolish houses traits what can be interpreted as boolean, but
 * internally may take on more values.  The classic example is two-bit
 * saturating counters (TwoBitCounters), but there are more
 * variations. Key is the convertion to and fro boolean as well as an
 * `update` that nudges the value in a particular direction.
 */

// TODO:
// - separate prediction and update, enabling modelling delayed updates

pub trait Boolish {
    fn update(&mut self, taken: bool) -> &mut Self;
    fn value(self) -> bool;
    fn new(b: bool) -> Self;
}

const _STRONGLY_NOT_TAKEN: i8 = 0;
const WEAKLY_NOT_TAKEN: i8 = 1;
const WEAKLY_TAKEN: i8 = 2;
const _STRONGLY_TAKEN: i8 = 3;
const SCALE: usize = 5;

// NB. Not using enums in order to use a bit encoding trick
#[derive(Copy, Clone)]
pub struct TwoBitCounter {
    counter: i8,
}
impl Boolish for TwoBitCounter {
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

        let new = self
            .counter
            .wrapping_add((taken as i8) * (2 << SCALE) - (1 << SCALE));
        let overflow_mask = (new << (5 - SCALE)) >> 7;
        self.counter = self.counter & overflow_mask | new & !overflow_mask;

        self
    }

    fn value(self) -> bool {
        WEAKLY_TAKEN << SCALE <= self.counter
    }

    fn new(b: bool) -> Self {
        TwoBitCounter {
            counter: if b {
                WEAKLY_TAKEN << SCALE
            } else {
                WEAKLY_NOT_TAKEN << SCALE
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempodence() {
        // Level 0 sanity - idempodence
        assert!(!TwoBitCounter::new(false).value());
        assert!(TwoBitCounter::new(true).value());
    }

    #[test]
    fn strengthening() {
        // Level 1 sanity - strengthening
        assert!(!TwoBitCounter::new(false).update(false).value());
        assert!(TwoBitCounter::new(true).update(true).value());
    }

    #[test]
    fn weak_update() {
        // Level 2 sanity - weak + change
        assert!(TwoBitCounter::new(false).update(true).value());
        assert!(!TwoBitCounter::new(true).update(false).value());
    }

    #[test]
    fn strong_update() {
        // Level 3 sanity - strong + change
        assert!(!TwoBitCounter::new(false).update(false).update(true).value());
        assert!(TwoBitCounter::new(true).update(true).update(false).value());

        // Level 4 sanity - strong + change*2
        assert!(TwoBitCounter::new(false)
            .update(false)
            .update(true)
            .update(true)
            .value());
        assert!(!TwoBitCounter::new(true)
            .update(true)
            .update(false)
            .update(false)
            .value());
    }
}

#[derive(Copy, Clone)]
pub enum Confidence {
    Weak,
    Fair,
    Strong,
    Conviction,
}

pub struct ProbablyBool {
    value: bool,
    confidence: Confidence,
}

impl Boolish for ProbablyBool {
    fn update(&mut self, new_value: bool) -> &mut Self {
        use Confidence::*;
        self.confidence = if self.value == new_value {
            /* Strengthen */
            match self.confidence {
                Weak => Fair,
                Fair if lucky_die_roll() => Strong,
                Strong if lucky_die_roll() => Conviction,
                _ => self.confidence,
            }
        } else {
            /*
             * Weaken. The probabilistic behavior is asymmetric as we
             * exit out of the high confidence on any negative result
             */
            match self.confidence {
                Weak => {
                    self.value = new_value;
                    Weak
                }
                Fair => Weak,
                Strong | Conviction => Fair,
            }
        };

        self
    }

    fn value(self) -> bool {
        self.value
    }

    fn new(value: bool) -> Self {
        Self {
            value,
            confidence: Confidence::Weak,
        }
    }
}

impl ProbablyBool {
    #[allow(dead_code)]
    fn confident(self) -> bool {
        !matches!(self.confidence, Confidence::Weak)
    }

    #[allow(dead_code)]
    fn highly_confident(self) -> bool {
        matches!(self.confidence, Confidence::Conviction)
    }
}

fn lucky_die_roll() -> bool {
    rand::thread_rng().gen_range(1..101) == 42
}
