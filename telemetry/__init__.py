"""The Telemetry kit's reference pipeline (Platform 6; SP-09-telemetry design).

The pipeline process: it receives the canonical permitted records that each producer's in-process adapter
(interpreter/telemetry.py, fortis-runtime's telemetry.rs) wrote after applying the privacy policy. It then handles
validation and the ledger, the archive, projection epochs, aggregation, acknowledgement and recovery, replay and
export. Its inputs are the schema registry, filled from programs' own observation declarations, and a telemetry plan
in YAML, which it also compiles into the adapters' policy. The production pipeline (DEL-55s) is held to the same
conformance suite.
"""
