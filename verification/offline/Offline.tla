------------------------------ MODULE Offline ------------------------------
EXTENDS FiniteSets

CONSTANTS FailClosed, RetireHelper, IsolateHelperAudio, ExternalDownloadChild, SkipCanceledRetirement
VARIABLES phase, modelPresent, sealed, running, confined, audio,
          helperReady, transferring, canceled, cleanupDone

vars == <<phase, modelPresent, sealed, running, confined, audio,
          helperReady, transferring, canceled, cleanupDone>>
C == "controller"
H == "helper"
I == "inference"
Processes == {C, H, I}
Downloaders == {H}
Operational == {"idle", "recording", "download", "verify", "retire", "wait"}

Init ==
    /\ phase = "boot"
    /\ modelPresent \in BOOLEAN
    /\ sealed = FALSE
    /\ running = {C}
    /\ confined = {}
    /\ audio = {}
    /\ helperReady = FALSE
    /\ transferring = FALSE
    /\ canceled = FALSE
    /\ cleanupDone = FALSE

ChooseStartup ==
    /\ phase = "boot"
    /\ phase' = IF modelPresent THEN "lock" ELSE "helper"
    /\ running' = IF modelPresent THEN running ELSE running \cup {H}
    /\ UNCHANGED <<modelPresent, sealed, confined, audio, helperReady,
                   transferring, canceled, cleanupDone>>

HelperReady ==
    /\ phase = "helper" /\ H \in running
    /\ helperReady' = TRUE /\ phase' = "lock"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   transferring, canceled, cleanupDone>>

LockSuccess ==
    /\ phase = "lock"
    /\ confined' = confined \cup {C}
    /\ sealed' = modelPresent /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, running, audio, helperReady,
                   transferring, canceled, cleanupDone>>

LockFailure ==
    /\ phase = "lock"
    /\ phase' = IF FailClosed THEN "dead" ELSE "idle"
    /\ running' = IF FailClosed THEN running \ {C} ELSE running
    /\ sealed' = IF FailClosed THEN FALSE ELSE modelPresent
    /\ UNCHANGED <<modelPresent, confined, audio, helperReady,
                   transferring, canceled, cleanupDone>>

BeginDownload ==
    /\ phase = "idle" /\ ~sealed /\ H \in running /\ helperReady
    /\ phase' = "download" /\ canceled' = FALSE
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, transferring, cleanupDone>>

StartTransfer ==
    /\ phase = "download" /\ H \in running
    /\ ~transferring /\ ~canceled
    /\ transferring' = TRUE
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined, audio,
                   helperReady, canceled, cleanupDone>>

Cancel ==
    /\ phase = "download" /\ ~canceled
    /\ canceled' = TRUE
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined, audio,
                   helperReady, transferring, cleanupDone>>

CancelCompletes ==
    /\ phase = "download" /\ canceled /\ H \in running
    /\ transferring' = FALSE /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio, helperReady,
                   canceled, cleanupDone>>

TransferCompletes ==
    /\ phase = "download" /\ transferring /\ ~canceled /\ H \in running
    /\ transferring' = FALSE /\ phase' = "verify"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio, helperReady,
                   canceled, cleanupDone>>

VerifySuccess ==
    /\ phase = "verify" /\ H \in running
    /\ modelPresent' = TRUE /\ phase' = "retire"
    /\ UNCHANGED <<sealed, running, confined, audio, helperReady,
                   transferring, canceled, cleanupDone>>

VerifyFailure ==
    /\ phase = "verify" /\ H \in running
    /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, transferring, canceled, cleanupDone>>

CancelPublishedResult ==
    /\ phase = "retire" /\ H \in running
    /\ phase' = IF SkipCanceledRetirement THEN "idle" ELSE "retire"
    /\ ~canceled /\ canceled' = TRUE
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, transferring, cleanupDone>>

DropDownloadHelper ==
    /\ phase = "retire"
    /\ phase' = IF RetireHelper THEN "wait" ELSE "idle"
    /\ sealed' = ~RetireHelper
    /\ running' = IF RetireHelper THEN running \ Downloaders ELSE running
    /\ UNCHANGED <<modelPresent, confined, audio, helperReady,
                   transferring, canceled, cleanupDone>>

ReapDownloadHelper ==
    /\ phase = "wait" /\ H \notin running
    /\ phase' = "idle" /\ sealed' = TRUE /\ helperReady' = FALSE
    /\ UNCHANGED <<modelPresent, running, confined, audio,
                   transferring, canceled, cleanupDone>>

Record ==
    /\ phase = "idle" /\ C \in running
    /\ phase' = "recording" /\ audio' = audio \cup {C}
    /\ UNCHANGED <<modelPresent, sealed, running, confined, helperReady,
                   transferring, canceled, cleanupDone>>

Infer ==
    /\ phase = "recording" /\ modelPresent /\ I \notin running
    /\ running' = running \cup {I}
    /\ confined' = confined \cup {I}
    /\ audio' = audio \cup {I}
    /\ UNCHANGED <<phase, modelPresent, sealed, helperReady,
                   transferring, canceled, cleanupDone>>

FinishRecording ==
    /\ phase = "recording" /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, transferring, canceled, cleanupDone>>

InferenceExits ==
    /\ I \in running
    /\ running' = running \ {I} /\ audio' = audio \ {I}
    /\ UNCHANGED <<phase, modelPresent, sealed, confined, helperReady,
                   transferring, canceled, cleanupDone>>

HelperReadsAudio ==
    /\ H \in running /\ helperReady /\ ~IsolateHelperAudio
    /\ audio' = audio \cup {H}
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined,
                   helperReady, transferring, canceled, cleanupDone>>

ControllerExits ==
    /\ C \in running
    /\ phase' = "dead" /\ running' = running \ {C}
    /\ audio' = audio \ {C}
    /\ UNCHANGED <<modelPresent, sealed, confined, helperReady,
                   transferring, canceled, cleanupDone>>

ParentWatchdog ==
    /\ phase = "dead" /\ H \in running
    /\ running' = running \ {H}
    /\ transferring' = FALSE
    /\ audio' = audio \ {H} /\ cleanupDone' = TRUE
    /\ UNCHANGED <<phase, modelPresent, sealed, confined, helperReady, canceled>>

HelperKilled ==
    /\ H \in running /\ phase \in Operational \cup {"dead"}
    /\ running' = running \ {H} /\ audio' = audio \ {H}
    /\ helperReady' = FALSE
    /\ transferring' = IF ExternalDownloadChild THEN transferring ELSE FALSE
    /\ phase' = IF phase \in {"recording", "dead"} THEN phase ELSE "idle"
    /\ UNCHANGED <<modelPresent, sealed, confined, canceled, cleanupDone>>

Next == ChooseStartup \/ HelperReady \/ LockSuccess \/ LockFailure
     \/ BeginDownload \/ StartTransfer \/ Cancel \/ CancelCompletes
     \/ TransferCompletes \/ VerifySuccess \/ VerifyFailure
     \/ CancelPublishedResult
     \/ DropDownloadHelper \/ ReapDownloadHelper
     \/ Record \/ Infer \/ FinishRecording \/ InferenceExits
     \/ HelperReadsAudio \/ ControllerExits \/ ParentWatchdog \/ HelperKilled

TypeOK ==
    /\ phase \in {"boot", "helper", "lock", "idle", "recording", "download",
                  "verify", "retire", "wait", "dead"}
    /\ modelPresent \in BOOLEAN /\ sealed \in BOOLEAN
    /\ running \subseteq Processes /\ confined \subseteq Processes
    /\ audio \subseteq Processes /\ helperReady \in BOOLEAN
    /\ transferring \in BOOLEAN
    /\ canceled \in BOOLEAN /\ cleanupDone \in BOOLEAN

OperationalConfined == phase \in Operational => C \in confined
AudioConfined == audio \cap running \subseteq confined
OfflineNoDownloadAuthority == sealed => running \cap Downloaders = {} /\ ~transferring
HelperHasNoAudio == H \notin audio
InferenceConfined == I \in running => I \in confined
CleanupRevokesDownloadAuthority == cleanupDone => running \cap Downloaders = {} /\ ~transferring
DownloadHasLiveOwner == transferring => H \in running
UsableModelHasNoDownloadAuthority ==
    modelPresent /\ phase \in {"idle", "recording"} => running \cap Downloaders = {} /\ ~transferring

Spec == Init /\ [][Next]_vars
=============================================================================
