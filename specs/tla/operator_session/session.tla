---- MODULE session ----
EXTENDS TLC

States ==
    {
        "idle",
        "planning",
        "acting",
        "verifying",
        "waiting_for_ui",
        "waiting_for_user",
        "paused",
        "failed",
        "completed"
    }

AllowedPairs ==
    {
        <<"idle", "idle">>,
        <<"idle", "planning">>,
        <<"idle", "acting">>,
        <<"idle", "paused">>,
        <<"idle", "failed">>,
        <<"idle", "completed">>,

        <<"planning", "planning">>,
        <<"planning", "acting">>,
        <<"planning", "waiting_for_ui">>,
        <<"planning", "waiting_for_user">>,
        <<"planning", "paused">>,
        <<"planning", "failed">>,
        <<"planning", "completed">>,

        <<"acting", "acting">>,
        <<"acting", "verifying">>,
        <<"acting", "waiting_for_ui">>,
        <<"acting", "waiting_for_user">>,
        <<"acting", "paused">>,
        <<"acting", "failed">>,
        <<"acting", "completed">>,

        <<"verifying", "verifying">>,
        <<"verifying", "planning">>,
        <<"verifying", "acting">>,
        <<"verifying", "waiting_for_ui">>,
        <<"verifying", "waiting_for_user">>,
        <<"verifying", "paused">>,
        <<"verifying", "failed">>,
        <<"verifying", "completed">>,

        <<"waiting_for_ui", "waiting_for_ui">>,
        <<"waiting_for_ui", "acting">>,
        <<"waiting_for_ui", "verifying">>,
        <<"waiting_for_ui", "paused">>,
        <<"waiting_for_ui", "failed">>,
        <<"waiting_for_ui", "completed">>,

        <<"waiting_for_user", "waiting_for_user">>,
        <<"waiting_for_user", "planning">>,
        <<"waiting_for_user", "acting">>,
        <<"waiting_for_user", "paused">>,
        <<"waiting_for_user", "failed">>,
        <<"waiting_for_user", "completed">>,

        <<"paused", "paused">>,
        <<"paused", "planning">>,
        <<"paused", "failed">>,
        <<"paused", "completed">>,

        <<"failed", "failed">>,
        <<"failed", "planning">>,
        <<"failed", "completed">>,

        <<"completed", "completed">>,
        <<"completed", "planning">>
    }

VARIABLES previous, current

Vars == <<previous, current>>

Init ==
    /\ previous = "idle"
    /\ current = "idle"

Transition(nextState) ==
    /\ nextState \in States
    /\ <<current, nextState>> \in AllowedPairs
    /\ previous' = current
    /\ current' = nextState

Next ==
    \E nextState \in States: Transition(nextState)

TypeInv ==
    /\ previous \in States
    /\ current \in States

LegalTransition ==
    <<previous, current>> \in AllowedPairs

EventuallyCompletes ==
    current = "acting" ~> (current = "completed" \/ current = "failed")

Spec ==
    Init /\ [][Next]_Vars

=============================================================================
