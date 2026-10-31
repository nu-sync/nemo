export const title = 'Nemo local-only lifecycle';
const hasHelper = (v) => v.running.includes('helper');
export function state(v) {
    if (v.phase === 'dead') return hasHelper(v) ? 'Controller stopped; cleanup pending' : 'Stopped';
    if (v.phase === 'idle') {
        if (v.modelPresent) return 'Offline ready';
        return hasHelper(v) ? 'Model needed' : 'Helper stopped; reopen to retry';
    }
    const labels = {
        boot: 'Start', helper: 'Start download helper', lock: 'Apply local-only policy',
        download: 'Download model in helper',
        verify: 'Verify model',
        retire: v.canceled ? 'Late cancellation; retirement required' : 'Model published; retire helper',
        wait: 'Wait for helper exit', recording: 'Handle audio locally',
    };
    return labels[v.phase];
}
export function step(action, from, to) {
    if (action === 'ControllerExits' && from.phase !== 'download' && !(from.phase === 'idle' && !from.modelPresent && !hasHelper(from))) return null;
    if (action === 'Record' && !from.modelPresent) return null;
    if (action === 'FinishRecording' && !to.modelPresent) return null;
    if (action === 'HelperKilled' && !['download', 'retire', 'dead'].includes(from.phase)) return null;
    const labels = {
        ChooseStartup: from.modelPresent ? 'use cached model' : 'prepare model acquisition',
        HelperReady: 'helper confined; subprocess execution denied',
        LockSuccess: 'confinement succeeds', LockFailure: 'confinement fails',
        BeginDownload: 'request pinned model', StartTransfer: 'start in-process HTTP',
        CancelCompletes: 'transfer canceled; retry available', TransferCompletes: 'transfer completes',
        VerifySuccess: 'verify hashes and publish', VerifyFailure: 'verification fails',
        CancelPublishedResult: 'cancel before success is accepted',
        DropDownloadHelper: 'terminate helper', ReapDownloadHelper: 'observe helper exit',
        Record: 'start audio handling', FinishRecording: 'finish audio handling',
        ControllerExits: 'controller exits', ParentWatchdog: 'watchdog terminates helper',
        HelperKilled: 'helper dies; transfer ends with it',
    };
    return labels[action] ?? action;
}
