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

        let new = self.counter + ((taken as i8) * (2 << SCALE) - (1 << SCALE));
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
        assert_eq!(TwoBitCounter::new(false).value(), false);
        assert_eq!(TwoBitCounter::new(true).value(), true);
    }

    #[test]
    fn strengthening() {
        // Level 1 sanity - strengthening
        assert_eq!(TwoBitCounter::new(false).update(false).value(), false);
        assert_eq!(TwoBitCounter::new(true).update(true).value(), true);
    }

    #[test]
    fn weak_update() {
        // Level 2 sanity - weak + change
        assert_eq!(TwoBitCounter::new(false).update(true).value(), true);
        assert_eq!(TwoBitCounter::new(true).update(false).value(), false);
    }

    #[test]
    fn strong_update() {
        // Level 3 sanity - strong + change
        assert_eq!(
            TwoBitCounter::new(false).update(false).update(true).value(),
            false
        );
        assert_eq!(
            TwoBitCounter::new(true).update(true).update(false).value(),
            true
        );

        // Level 4 sanity - strong + change*2
        assert_eq!(
            TwoBitCounter::new(false)
                .update(false)
                .update(true)
                .update(true)
                .value(),
            true
        );
        assert_eq!(
            TwoBitCounter::new(true)
                .update(true)
                .update(false)
                .update(false)
                .value(),
            false
        );
    }
}

#[derive(Copy, Clone)]
pub enum MichaudBool {
    NotTaken100,
    NotTaken99,
    NotTaken1,
    NotTaken0,
    Taken0,
    Taken1,
    Taken99,
    Taken100,
}

impl Boolish for MichaudBool {
    fn update(&mut self, taken: bool) -> &mut Self {
        use MichaudBool::*;
        *self = if taken {
            match *self {
                NotTaken100 => NotTaken1, // No place for confidence!
                NotTaken99 => NotTaken1,
                NotTaken1 => NotTaken0,
                NotTaken0 => Taken0,
                Taken0 => Taken1,
                Taken1 => {
                    if lucky_die_roll() {
                        Taken99
                    } else {
                        Taken1
                    }
                }
                Taken99 => {
                    if lucky_die_roll() {
                        Taken100
                    } else {
                        Taken99
                    }
                }
                Taken100 => Taken100,
            }
        } else {
            match *self {
                NotTaken100 => NotTaken100,
                NotTaken99 => {
                    if lucky_die_roll() {
                        NotTaken100
                    } else {
                        NotTaken99
                    }
                }
                NotTaken1 => {
                    if lucky_die_roll() {
                        NotTaken99
                    } else {
                        NotTaken1
                    }
                }
                NotTaken0 => NotTaken1,
                Taken0 => NotTaken0,
                Taken1 => Taken0,
                Taken99 => Taken1,
                Taken100 => Taken1, // No place for confidence!
            }
        };

        self
    }

    fn value(self) -> bool {
        use MichaudBool::*;
        matches!(self, Taken100 | Taken99 | Taken1 | Taken0)
    }

    fn new(b: bool) -> Self {
        if b {
            MichaudBool::Taken0
        } else {
            MichaudBool::NotTaken0
        }
    }
}

impl MichaudBool {
    #[allow(dead_code)]
    fn confident(self) -> bool {
        matches!(self, MichaudBool::NotTaken100 | MichaudBool::Taken100)
    }
}

fn lucky_die_roll() -> bool {
    let magic_number = rand::thread_rng().gen_range(1..101);
    magic_number == 1
}
