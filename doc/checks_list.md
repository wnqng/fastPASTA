# Checks performed with the `check` subcommand

## Table of contents
- [Preliminary sanity checks](#prelimary-sanity-checks)
- [Running RDH checks](#rdh-running-checks)
- [ITS specific checks](#its-specific-checks)
  - [RDH sanity checks](#rdh-sanity-check-1)
  - [Payload sanity checks](#payload-sanity-checks)
  - [Payload running checks](#payload-running-checks)

## Prelimary sanity checks
> These checks are done to verify that the input data is scanned correctly, if any of them fail, data is skipped or if that is not possible, a fatal error is raised and processing stops
### RDH version and payload size (Performed in the `input module`)
1. `Once` The first 10 bytes of the input is read as an RDH0 and the version field is checked, if it is not 6 or 7, processing is stopped.

2. `Every RDH` The input scanner uses RDHs to navigate the data, and does one sanity check on the `offset_to_next` field. It subtracts the size of an RDH (64 bytes) from the value of the `offset_to_next` field, and checks that the result is not less than 0, and not more than 20 KB. If it fails, processing will stop.


### ITS Payload preprocessing (Performed in the `validation module`)
End of payload padding is checked, if it exceed 15 bytes, an error is raised and the payload is skipped, and the CDP payload FSM is reset.



# RDH checks (Performed in the `validation module`)
## RDH running checks
### Check stop_bit, page_counter, orbit, packet_counter, FeeId, trigger_type (Performed in the `validation module`)
Uses the value of the stop_bit to determine if the page_counter is expected to increment or reset to 0.

* `If stop_bit == 0`
  * Check page_counter == expected_page_counter
  * Increment expected_page_counter
* `If stop_bit == 1`
  * Check page_counter == expected_page_counter
  * Reset expected_page_counter to 0
  * Check next RDH's orbit is different from the current RDH's orbit
* `If page_counter != 0` check that these fields are same as previous RDH:
  * orbit
  * trigger
  * detector field
  * FeeID





# Sanity checks (Performed in the `validation module`)
If any of the following conditions are not met, the RDH fails the sanity check and one or more error messages is printed to stderr.
## RDH sanity check
* RDH0
  * Header ID equal to first Header ID seen during processing
  * header_size = 0x40
  * FeeID
    * 0 <= layer <= 6
    * 0 <= stave <= 47
    * reserved = 0
  * priority_bit = 0
  * reserved = 0
* RDH1
  * bc < 0xdeb
  * reserved = 0
* RDH2
  * stop_bit <= 1
  * trigger_type >= 1 AND all spare bits = 0
  * reserved = 0
* RDH3
  * reserved = 0 `includes reserved 23:4 in detector field`
* dw <= 1
* data_format <= 2


# ITS specific checks
## RDH sanity check
* RDH0
  * system_id = 0x20 `ITS system ID`

## Payload sanity checks
All ID checks are made based on the FSM illustrated in the section [Payload running checks](#payload-running-checks).
### Status Words
#### IHW
* id = 0xE0
* reserved = 0

#### TDH
* id = 0xE8
* reserved = 0
* trigger_type != 0 `OR` internal_trigger != 0

#### TDT
* id = 0xF0
* reserved = 0

#### DDW0
* id = 0xE4
* reserved = 0
* index >= 1

### Data Words
Checks that the ID is a valid ID for IL, ML or OL.

If any of the following checks passes, it is considered valid.

0x20 <= ID <= 0x28 `IL`

0x43 <= ID <= 0x46 `ML`

0x48 <= ID <= 0x4B `ML`

0x53 <= ID <= 0x56 `ML`

0x58 <= ID <= 0x5B `ML`

0x40 <= ID <= 0x46 `OL`

0x48 <= ID <= 0x4E `OL`

0x50 <= ID <= 0x56 `OL`

0x58 <= ID <= 0x5E `OL`

## Payload running checks
Before each payload is checked, the rdh for that payload is set as the current rdh. A state machine (see below) is used to keep track of which words are expected, and sanity checks are performed on the each word (sanity checks are listed further down this document).

Additional checks related to state:
* `When:` Word is DDW0
  * RDH stop_bit == 1
  * RDH pages_counter > 0
* `When:` Word is IHW (not in continuation substate)
  * RDH stop_bit == 0
* `When:` TDH following a TDT with packet_done == 1
  * TDH internal_trigger == 1
  * TDH continuation == 0
  * TDH trigger_bc > previous TDH
* `When:` TDH following a TDT with packet_done == 0
  * TDH continuation == 1
* `When:` CDW where user_field != previous CDW user_field
  * CDW index == 0
* `When:` Data Word observed
  * lane in IHW active_lanes
  * `When:` OB data word:
    * Input connector number < 7


Certain transitions are ambigious (marked by yellow notes), these are resolved based on the ID of the next received GBT word.
![CDP FSM for validation](CDP_payload_StateMachine%20(continuous%20mode).png)
