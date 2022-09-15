//! Parity hash experiments
//!
//! Using hash parity to eliminate the extra writes needed due to swapping in MCUboot.
//!
//! Credits:
//! - Brendan Moran came up with the idea of using parity to recover when one page has been lost.
//! - Geraint Luff suggested using a Merkle tree to find the recovery point
//! - David Brown came up with the idea of detecting the recovery point instead of storing the
//!   state, as well as the importance of skipping pages where the two images have identical
//!   contents.

// TODO: Implement skip map, and store it.

// Turn this off once more code is written.
#![allow(dead_code)]

// use sha2::{Digest, Sha256};
// use std::io::Write;

use flash::{Flash};

mod flash;
mod pdump;

type Result<T> = anyhow::Result<T>;

fn main() -> Result<()> {
    let flash = Flash::build([16, 16], [14, 13])?;
    let _ = flash;
    println!("flash: {}", flash);
    // recovery(0)?;
    Ok(())
}

/*
/// Perform a swap with the given stopping point, and attempt recovery.
fn recovery(stop: usize) -> Result<()> {
    let mut work = Status::new(6)?;

    work.stop = Some(stop);
    if let SwapResult::Finished = work.swap() {
        panic!("Too many steps for work to complete");
    }

    // TODO: Allow for multiple stopping points.
    work.stop = None;
    work.recover()?;
    work.final_check();
    Ok(())
}

/// Indicates how a swap operation ran.
enum SwapResult {
    Finished,
    Interrupted,
}

#[derive(Debug)]
struct PageLocation {
    slot: usize,
    index: usize,
}

/// For this experiment, we don't try to map the status into the flash itself, but merely store it
/// in memory.
#[derive(Debug)]
struct Status {
    slots: [Slot; 2],
    root: Vec<u8>,
    parity: Vec<u8>,

    /// What step in the swap process are we on.
    step: usize,

    /// What step in the swap process should we stop at.
    stop: Option<usize>,

    /// When interrupted, this indicates where we expect the resume to continue.
    resume: Option<PageLocation>,
}

// Nice display for Page.
/*
impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(pos) = self.payload.iter().position(|&x| x == 0xFF) {
            write!(f, "[{}]", str::from_utf8(&self.payload[0..pos]).unwrap())?;
        } else {
            write!(f, "[unknown page]")?;
        }
        Ok(())
    }
}
*/

impl Slot {
    /// Compute the Merkel root for the data in the slot.
    /// TODO: Don't return and copy result, twice
    fn compute_root(&self) -> Result<Vec<u8>> {
        let mut state = Sha256::new();
        for (index, b) in self.data.iter().enumerate() {
            state.update(&b.digest(PageLocation {
                slot: self.index,
                index,
            })?);
        }
        Ok(state.finalize().to_vec())
    }

    /// Compute a parity block for the entire image.
    fn compute_parity(&self) -> Vec<u8> {
        let mut result = vec![0u8; PAGE_SIZE];

        for b in &self.data {
            for (i, &bt) in b.payload.iter().enumerate() {
                result[i] ^= bt;
            }
        }
        result
    }
}

impl Status {
    fn new(size: usize) -> Result<Status> {
        let slot0 = Slot::new(0, size);
        let slot1 = Slot::new(1, size);
        let root = slot1.compute_root()?;
        let parity = slot0.compute_parity();

        Ok(Status {
            slots: [slot0, slot1],
            root,
            parity,
            step: 0,
            stop: None,
            resume: None,
        })
    }

    fn swap(&mut self) -> SwapResult {
        // TODO: Support different sizes for the slots.
        assert_eq!(self.slots[0].data.len(), self.slots[1].data.len());

        // We need two buffers for the operation.
        let mut abuf = vec![0u8; PAGE_SIZE];
        let mut bbuf = vec![0u8; PAGE_SIZE];

        for sec in 0..self.slots[0].data.len() {
            // We need to re-borrow this value each time we access the field.  This macro helps
            // keep the reference short.
            macro_rules! slot {
                ($index:literal) => {
                    self.slots[$index].data[sec]
                };
            }

            slot!(0).read(&mut abuf);
            slot!(1).read(&mut bbuf);

            // We consume 4 steps here.  One is before the erase, one after the write, and in both
            // cases, we make sure that we restart after the write.

            self.step += 1;
            if self.is_stop() {
                slot!(0).partial_erase();
                self.resume = Some(PageLocation {
                    slot: 0,
                    index: sec,
                });
                return SwapResult::Interrupted;
            } else {
                slot!(0).erase();
            }

            self.step += 1;
            if self.is_stop() {
                slot!(0).partial_write(&bbuf);
                self.resume = Some(PageLocation {
                    slot: 0,
                    index: sec,
                });
                return SwapResult::Interrupted;
            } else {
                slot!(0).write(&bbuf);
            }

            self.step += 1;
            if self.is_stop() {
                slot!(1).partial_erase();
                self.resume = Some(PageLocation {
                    slot: 1,
                    index: sec,
                });
                return SwapResult::Interrupted;
            } else {
                slot!(1).erase();
            }

            self.step += 1;
            if self.is_stop() {
                slot!(1).partial_write(&abuf);
                self.resume = Some(PageLocation {
                    slot: 1,
                    index: sec,
                });
                return SwapResult::Interrupted;
            } else {
                slot!(1).write(&abuf);
            }
        }

        SwapResult::Finished
    }

    /// Perform a startup recovery.  Finds the recovery point, and continues the swapping.
    fn recover(&mut self) -> Result<()> {
        let loc = self.find_recovery()?;
        println!("loc: {:?}", loc);
        Ok(())
    }

    /// Scan the device for the recovery point.  If we have enough RAM for
    /// hashes for every block, we can be a little more robust, not having to
    /// rely on the possibility of consecutive reads of the same data returning
    /// something different.
    fn find_recovery(&self) -> Result<PageLocation> {
        unimplemented!()
    }

    /// Compute a final check to ensure that the given swap has completed.
    fn final_check(&self) {
        for sec in 0..self.slots[0].data.len() {
            self.slots[0].data[sec].check(PageLocation {
                slot: 1,
                index: sec,
            });
            self.slots[1].data[sec].check(PageLocation {
                slot: 0,
                index: sec,
            });
        }
    }

    /// Is our position such that we should stop.
    fn is_stop(&self) -> bool {
        if let Some(stop) = self.stop {
            self.step > stop
        } else {
            false
        }
    }
}
*/
