---- MODULE owner ----
EXTENDS FiniteSets, TLC

CONSTANT Hosts

VARIABLES activeOwner, conflictSeen

Vars == <<activeOwner, conflictSeen>>

Init ==
    /\ activeOwner = {}
    /\ conflictSeen = FALSE

Claim(h) ==
    /\ h \in Hosts
    /\ activeOwner = {}
    /\ activeOwner' = {h}
    /\ conflictSeen' = FALSE

Reclaim(h) ==
    /\ h \in Hosts
    /\ activeOwner = {h}
    /\ UNCHANGED activeOwner
    /\ conflictSeen' = FALSE

ObserveConflict(h) ==
    /\ h \in Hosts
    /\ activeOwner /= {}
    /\ h \notin activeOwner
    /\ UNCHANGED activeOwner
    /\ conflictSeen' = TRUE

Release(h) ==
    /\ h \in activeOwner
    /\ activeOwner' = {}
    /\ conflictSeen' = FALSE

Recover ==
    /\ conflictSeen = TRUE
    /\ UNCHANGED activeOwner
    /\ conflictSeen' = FALSE

Next ==
    \E h \in Hosts:
        Claim(h)
        \/ Reclaim(h)
        \/ ObserveConflict(h)
        \/ Release(h)
    \/ Recover

TypeInv ==
    /\ activeOwner \subseteq Hosts
    /\ conflictSeen \in BOOLEAN

SingleOwner ==
    Cardinality(activeOwner) <= 1

ConflictOnlyWhenOwned ==
    conflictSeen => activeOwner /= {}

Spec ==
    Init /\ [][Next]_Vars

RecoveryPossible ==
    conflictSeen ~> ~conflictSeen

=============================================================================
