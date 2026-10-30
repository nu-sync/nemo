------------------------------ MODULE Offline ------------------------------
EXTENDS FiniteSets

CONSTANTS FailClosed, RetireHelper, IsolateHelperAudio, KillWholeGroup
VARIABLES phase, modelPresent, sealed, running, confined, audio,
          helperReady, curlSlot, canceled, cleanupDone

vars == <<phase, modelPresent, sealed, running, confined, audio,
          helperReady, curlSlot, canceled, cleanupDone>>
C == "controller"
H == "helper"
D == "curl"
I == "inference"
Processes == {C, H, D, I}
Downloaders == {H, D}
Operational == {"idle", "recording", "download", "verify", "retire", "wait"}

Init ==
    /\ phase = "boot"
    /\ modelPresent \in BOOLEAN
    /\ sealed = FALSE
    /\ running = {C}
    /\ confined = {}
    /\ audio = {}
    /\ helperReady = FALSE
    /\ curlSlot = "none"
    /\ canceled = FALSE
    /\ cleanupDone = FALSE

ChooseStartup ==
    /\ phase = "boot"
    /\ phase' = IF modelPresent THEN "lock" ELSE "helper"
    /\ running' = IF modelPresent THEN running ELSE running \cup {H}
    /\ UNCHANGED <<modelPresent, sealed, confined, audio, helperReady,
                   curlSlot, canceled, cleanupDone>>

HelperReady ==
    /\ phase = "helper" /\ H \in running
    /\ helperReady' = TRUE /\ phase' = "lock"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   curlSlot, canceled, cleanupDone>>

LockSuccess ==
    /\ phase = "lock"
    /\ confined' = confined \cup {C}
    /\ sealed' = modelPresent /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, running, audio, helperReady,
                   curlSlot, canceled, cleanupDone>>

LockFailure ==
    /\ phase = "lock"
    /\ phase' = IF FailClosed THEN "dead" ELSE "idle"
    /\ running' = IF FailClosed THEN running \ {C} ELSE running
    /\ sealed' = IF FailClosed THEN FALSE ELSE modelPresent
    /\ UNCHANGED <<modelPresent, confined, audio, helperReady,
                   curlSlot, canceled, cleanupDone>>

BeginDownload ==
    /\ phase = "idle" /\ ~sealed /\ H \in running /\ helperReady
    /\ phase' = "download" /\ canceled' = FALSE
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, curlSlot, cleanupDone>>

SpawnCurl ==
    /\ phase = "download" /\ H \in running
    /\ curlSlot = "none" /\ ~canceled
    /\ running' = running \cup {D} /\ curlSlot' = "unregistered"
    /\ UNCHANGED <<phase, modelPresent, sealed, confined, audio,
                   helperReady, canceled, cleanupDone>>

RegisterCurl ==
    /\ phase = "download" /\ H \in running
    /\ curlSlot = "unregistered" /\ ~canceled
    /\ curlSlot' = "registered"
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined, audio,
                   helperReady, canceled, cleanupDone>>

Cancel ==
    /\ phase = "download" /\ ~canceled
    /\ canceled' = TRUE
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined, audio,
                   helperReady, curlSlot, cleanupDone>>

CancelCompletes ==
    /\ phase = "download" /\ canceled /\ H \in running
    /\ running' = running \ {D} /\ curlSlot' = "none" /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, confined, audio, helperReady,
                   canceled, cleanupDone>>

CurlCompletes ==
    /\ phase = "download" /\ curlSlot = "registered" /\ ~canceled
    /\ D \in running /\ H \in running
    /\ running' = running \ {D} /\ curlSlot' = "none" /\ phase' = "verify"
    /\ UNCHANGED <<modelPresent, sealed, confined, audio, helperReady,
                   canceled, cleanupDone>>

VerifySuccess ==
    /\ phase = "verify" /\ H \in running
    /\ modelPresent' = TRUE /\ phase' = "retire"
    /\ UNCHANGED <<sealed, running, confined, audio, helperReady,
                   curlSlot, canceled, cleanupDone>>

VerifyFailure ==
    /\ phase = "verify" /\ H \in running
    /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, curlSlot, canceled, cleanupDone>>

CancelPublishedResult ==
    /\ phase = "retire" /\ H \in running
    /\ phase' = "idle" /\ canceled' = TRUE
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, curlSlot, cleanupDone>>

DropDownloadHelper ==
    /\ phase = "retire"
    /\ phase' = IF RetireHelper THEN "wait" ELSE "idle"
    /\ sealed' = ~RetireHelper
    /\ running' = IF RetireHelper THEN running \ Downloaders ELSE running
    /\ UNCHANGED <<modelPresent, confined, audio, helperReady,
                   curlSlot, canceled, cleanupDone>>

ReapDownloadHelper ==
    /\ phase = "wait" /\ H \notin running
    /\ phase' = "idle" /\ sealed' = TRUE /\ helperReady' = FALSE
    /\ UNCHANGED <<modelPresent, running, confined, audio,
                   curlSlot, canceled, cleanupDone>>

Record ==
    /\ phase = "idle" /\ C \in running
    /\ phase' = "recording" /\ audio' = audio \cup {C}
    /\ UNCHANGED <<modelPresent, sealed, running, confined, helperReady,
                   curlSlot, canceled, cleanupDone>>

Infer ==
    /\ phase = "recording" /\ modelPresent /\ I \notin running
    /\ running' = running \cup {I}
    /\ confined' = confined \cup {I}
    /\ audio' = audio \cup {I}
    /\ UNCHANGED <<phase, modelPresent, sealed, helperReady,
                   curlSlot, canceled, cleanupDone>>

FinishRecording ==
    /\ phase = "recording" /\ phase' = "idle"
    /\ UNCHANGED <<modelPresent, sealed, running, confined, audio,
                   helperReady, curlSlot, canceled, cleanupDone>>

InferenceExits ==
    /\ I \in running
    /\ running' = running \ {I} /\ audio' = audio \ {I}
    /\ UNCHANGED <<phase, modelPresent, sealed, confined, helperReady,
                   curlSlot, canceled, cleanupDone>>

HelperReadsAudio ==
    /\ H \in running /\ helperReady /\ ~IsolateHelperAudio
    /\ audio' = audio \cup {H}
    /\ UNCHANGED <<phase, modelPresent, sealed, running, confined,
                   helperReady, curlSlot, canceled, cleanupDone>>

ControllerExits ==
    /\ C \in running
    /\ phase' = "dead" /\ running' = running \ {C}
    /\ audio' = audio \ {C}
    /\ UNCHANGED <<modelPresent, sealed, confined, helperReady,
                   curlSlot, canceled, cleanupDone>>

ParentWatchdog ==
    /\ phase = "dead" /\ H \in running
    /\ running' = running \ (IF KillWholeGroup THEN Downloaders
                              ELSE IF curlSlot = "registered" THEN Downloaders
                              ELSE {H})
    /\ audio' = audio \ {H} /\ cleanupDone' = TRUE
    /\ UNCHANGED <<phase, modelPresent, sealed, confined, helperReady,
                   curlSlot, canceled>>

HelperKilled ==
    /\ H \in running /\ phase \in Operational \cup {"dead"}
    /\ running' = running \ {H} /\ audio' = audio \ {H}
    /\ helperReady' = FALSE
    /\ phase' = IF phase \in {"recording", "dead"} THEN phase ELSE "idle"
    /\ UNCHANGED <<modelPresent, sealed, confined, curlSlot,
                   canceled, cleanupDone>>

Next == ChooseStartup \/ HelperReady \/ LockSuccess \/ LockFailure
     \/ BeginDownload \/ SpawnCurl \/ RegisterCurl \/ Cancel \/ CancelCompletes
     \/ CurlCompletes \/ VerifySuccess \/ VerifyFailure
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
    /\ curlSlot \in {"none", "unregistered", "registered"}
    /\ canceled \in BOOLEAN /\ cleanupDone \in BOOLEAN

OperationalConfined == phase \in Operational => C \in confined
AudioConfined == audio \cap running \subseteq confined
OfflineNoDownloadAuthority == sealed => running \cap Downloaders = {}
HelperHasNoAudio == H \notin audio
InferenceConfined == I \in running => I \in confined
CleanupRevokesDownloadAuthority == cleanupDone => running \cap Downloaders = {}
DownloadHasLiveOwner == D \in running => H \in running
UsableModelHasNoDownloadAuthority ==
    modelPresent /\ phase \in {"idle", "recording"} => running \cap Downloaders = {}

Spec == Init /\ [][Next]_vars
=============================================================================
