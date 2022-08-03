# Introduction

At the time of writing, MCUboot supports several swap implementations, with two
of them supporting the notion of "swap", where the contents of the two slots are
exchanged, so that it is possible to rever to an older image.

The oldest of these, known as "swap scratch", uses the two slots, and additional
a scratch partition.  In order to allow for safe upgrades, data is cycled
through the scratch partition in pieces, in order to allow the operation to
always be restarted in the case of interruption, never relying on the contents
of volatile memory.

A newer algorithm, known as "swap move", eliminates the need for this scratch
partition.  Instead, the first slot has to be a single block larger than the
other.  The upgrade happens in two stages.  First the contents of the first slot
are shifted downward in flash, leaving the first block empty.  Then the swap
proceeds, making use of this extra block to hold the data.  Instead of wearing
the flash evenly, the wear is spread across the first partition.  With the write
needed for an upgrade, the wear is even between the two partitions.

Both of these algorithms make use of a large block of status locations in the
end of each partition.  Using techniques appropriate for the given underlying
flash technology, it records each step of the operation.  For worst case, with
8-byte write alignment, this needs 24 bytes of status area for each block that
is swapped.

# Smaller status

There are two aspects of this upgrade operation that we seek to improve.  The
first is the need for an extra place to write data.  Although, as we see, we
can't eliminate the need for the extra block in the first partition, we can
eliminate the number of flash operations needed to perform an upgrade.

Second, we will be able to eliminate the need to write to a status area between
each flash operation.  This becomes especially important as some flash devices
have a large write size, as much as a whole 512-byte block.

To make this possible, we will need to make use of a few techniques that, when
combined, allow the swap operation to be done naively.

## Key 1: Don't do redundant work

Often when upgrading firmware, there will be blocks where the contents of the
new image are the same as the same block in the old image.  Currently, these are
upgraded anyway.  However, in order to be able to recover state, it can be
impossible to tell if this swap had started, or had run to completion.  As such,
as part of the swap preparation, we will build a bitmap of every block, with a
bit set for any blocks where the old image and the new image has identical
contents.  The rest of the operations described here will act is if these blocks
don't exist, and will be skipped entirely.  There will never need to be a
decision as to whether to recover the swap of these blocks, because the swap is
never done.

## Key 2: Rolling Integrity

Before any operation is started, the algorithm will compute a rolling
Merkel-like tree of the contents of the two slots.  This is computed as follows:

```
   H(key || block 0) XOR
   H(key || block 1) XOR
   ...
   H(key || block n)
```

Effectively, this is the hash of each block, independently, all xored together.
This gives us a fairly strong integrity check of the contents of the image, but
also allows us to replace the calculation of a single block by merely XORing out
the old hash and XORing in a new hash.  We will see, later, how this is used to
determine the recovery position.  The recovery process will always use and
compute the integrity of slot 1, and there are a few special cases that need to
be resolved using the integrity for slot 0.

## Key 3: Slot 0 Parity

A page in slot 0 will be used to hold a parity of the contents of the previous
blocks in the slot.  For this computation, byte 0 of the partity block will
consist of the XOR-sum of byte 0 across all of the data blocks in slot 0.  This
continues with byte 1, through all of the bytes within the block.  By using this
technique, we will be able to recover the contents of a single block, provided
we know which block it is that has incorrect contents.  This combined with the
above rolling integrity will allow recovery.

# The Upgrade Process

Before the upgrade starts, the rolling integrity for slots 0, and 1, as well as
the partity block for slot 0 are computed, and stored in the status area at the
end of the slot 0.  Depending on the flash type, how the parity is updated
varies:

- For NOR-type flashes, similar to those already supported by the earlier swap
  algorithms, the status is written at the end of slot 0.  The copy-done flag will
  be written when the entire operation has completed, to indicate the swap is
  finished.

- For flash devices that write in larger units, an initial status is written in
  the last page of slot 0.  When the upgrade has completed, it is upgraded in two
  stages, first by writing a completion page at the end of slot 1, and then
  finally by writing the completion page at the end of slot 0.  This allows the
  bootable image and these last few state steps to be restarted if interrupted.

Once the status has been written out, the upgrade can start.  The swap proceeds
in a straightforward way, first erasing a block in slot 0, then writing the data
from slot 1 into this erased block, then by erasing the block in slot 1, and
writing (from RAM) the contents from slot 0 into the block in slot 1.

After all of the blocks are swapped in this way, the status is updated, as
above, to indicate the swap has completed.

# Recovery

Given the 4 steps to swapping a given page, there are four possible situations
the swap could have been interrupted during.  The goal of recovery is to
determine which of these states we were in, and to complete the operations.

To summarize, the four steps are:

1.  Erase slot 0
2.  Write slot 0
3.  Erase slot 1
4.  Write slot 1

On many flash devices, it is not always possible to distinguish between a flash
operation being interrupted near the end of its operation, and it being
completed.  However, we can take evidence that the subsequent operation has
started to indicate that a given operation has completed.  Also note that
through the bulk of the device, these operations are performed on each device,
meaning that step 1 immediately follows step 4 of the previous block index.

Once step 1 has started, the flash will look something like this:

```
    B A
    B A
 -> X B
    A B
    A B
```

with the marked block as the one that has started erasing.  Since the recovery
process only looks for the proper contents of the 'B' image, this is ambiguous
between this and the last configuration, where the previous 'A' is in the
process of being written.  As such this case requires 'A' position recovery,
which is described below.

Once we've determined this is the last state, we need to recover the contents of
the 'A' block (it doesn't need to be written yet), and then we continue the
upgrade by erasing this block, and copying the 'B' block data onto it, and then
erasing the slot 1 block, and writing the 'A' contents.

When step 3 and 4 are in process, the flash will look something like one of the
following.

```
    B A
    B A
    B B
    A B
    A B
```

```
    B A
    B A
    B X
    A B
    A B
```

```
    B A
    B A
    B A
    A B
    A B
```

For the first two cases, we need to recover the contents of the 'A' block, and
finish erasing and writing it to slot 0.

To distinguish between these cases, and the first case, above, we need to use
'A' position recovery to distinguish between these cases, and the other
configuration.

## B Zipping

To perform recovery, we want to determine the last place in slot 1 that still
has the B contents written to it.  We will begin by hashing all of slot 1,
XORing them together and comparing with the result.  Then, until this is
correct, we will XOR slot 1 and XOR in the slot 0 hash one at a time, until the
root hash matches.

## Summary of all interruption points.

### First operation.

```
  0:  X   B1
  1:  A2  B2
  2:  A3  B3
  3:  A4  B4
```

This is the state of the flash when there is an interruption of step one, or
step 1 has completed by step two has not started.  In this case, we need to
recovery the contents of slot 0, first block.

### second and third operation.

```
  0:  B1  X
  1:  A2  B2
  2:  A3  B3
  3:  A4  B4
```

```
  0:  B1  A1
  1:  X   B2
  2:  A3  B3
  3:  A4  B4
```

When we have detected B1 fully written in slot 0, and not written in slot 1, we
then need to distinguish between the two states shown here.  We first check the
case where A2 is still written to slot 0.  If this is the case, we can assume
that slot 1, block 0 was in the process of being written, and continue the
upgrade by copying B2 to this block.  There isn't a need to recover A in this
case.  We know that the erase or write of slot 0, block 1 has started, and
therefore that A1 was fully written.

### last recovery

```
  0: B1  A1
  1: B2  X
  2: A3  B3
  3: A4  B4
```

```
  0: B1  A1
  1: B2  A2
  2: X   B3
  3: A4  B4
```

This last recovery state, it turns out is identical to the previous one.
