"""Read lifecycle state without treating readiness labels as recording outcomes."""

import json
import socket


def control(data, command):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(3)
        connection.connect(str(data / "run/control.sock"))
        connection.sendall((command + "\n").encode())
        return connection.makefile("r").readline().strip()


def lifecycle_status(query):
    response = query("readiness")
    if response == "unknown command":
        return query("status")
    activity = json.loads(response)["activity"]
    active = {
        "checking": "Checking readiness", "opening_microphone": "Opening microphone",
        "recording": "Recording", "loading_model": "Loading model",
        "downloading_model": "Downloading model", "transcribing": "Transcribing",
        "canceling": "Canceling",
    }
    if activity != "idle":
        return active[activity]
    outcome = query("status").rsplit(" · ", 1)[-1]
    return {"Idle": "Ready", "Transcript delivered": "Ready",
            "Model unloaded": "Ready · model unloaded"}.get(outcome, outcome)
