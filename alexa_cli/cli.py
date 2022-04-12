#!/usr/bin/env python
"""
Alexa CLI
-------------
A hacky but small script that acts as an Alexa Voice Service client allowing command-line access to Alexa.
Usage:
---
$ python cli.py "what time is it"
its nine fifteen p m
$ python cli.py --help
$ python cli.py --configure # Setup AVS Client ID, Secret, and Program Name
$ python cli.py "what time is it" --verbose --output=./output_dir_of_artifacts
...
-------------
"""

import os
import sys
import uuid
import fire
import json
import shutil
import ffmpeg
import socket
import requests
import subprocess
from absl import logging
from os.path import expanduser
import speech_recognition as sr
from alexa_client import AlexaClient
from flask import Flask, request, redirect, send_from_directory, has_request_context

FlaskApp = Flask(__name__)

def load_config():
    """ Loads the configuration from the current director or home directory"""
    p = "./config.json"
    if not os.path.exists(p):
        p = expanduser("~/.alexa/config.json")

    if not os.path.exists(p):
        raise Exception(f"Cannot find config.json {p}")

    with open(p) as f:
        config = json.load(f)

    return config

def read_refresh_token():
    p = "./tokens.json"
    if not os.path.exists(p):
        p = expanduser("~/.alexa/tokens.json")

    if not os.path.exists(p):
        raise Exception(f"Cannot find tokens.json {p}. Please run with '--configure'")

    with open(p) as f:
        tokens = json.load(f)

    return tokens["refresh_token"]


CONFIG = load_config()

class Server():
    def __init__(self, config):
        self.port = 8086
        if "port" in config.keys():
            self.port = config["port"]

        self.app = FlaskApp #gross

    def run(self):
        self.app.run(host="0.0.0.0", port=self.port)
        logging.debug(f"Server.run: started Flask server.")

@FlaskApp.route("/")
def home():
    return redirect(f"http://{request.host}/auth?state={socket.gethostname()}")

def get_redirect_uri(config):
    """ Redirects the default URI """
    redirect_uri = None
    if has_request_context():
        redirect_uri = f"http://{request.host}/auth"
    else:
        redirect_uri = f'http://localhost:{config["port"]}/auth'
    return redirect_uri

def get_tokens(config, code):
    redirect_uri = get_redirect_uri(config)
    payload = {'grant_type': 'authorization_code', 'code': code,
			'client_id': config["client_id"], 'client_secret': config["secret"], 'redirect_uri': redirect_uri}
   
    logging.debug(f"Getting Auth Token for Payload {payload}")

    url = "https://api.amazon.com/auth/o2/token"
    r = requests.post(url, data=payload)
    tokens = r.json()
    return tokens

def refresh_tokens(refresh_token):
    redirect_uri = get_redirect_uri()
    payload = {'grant_type': 'refresh_token', 'refresh_token': refresh_token,
               'client_id': CONFIG["client_id"], 'client_secret': CONFIG["secret"], 'redirect_uri': redirect_uri}
    
    logging.debug(f"refresh_tokens: Refreshing Token with payload")
    url = "https://api.amazon.com/auth/o2/token"
    r = requests.post(url, data=payload)
    tokens = r.json()
    logging.debug(f"refresh_tokens: Received refresh tokens {tokens}")
    return tokens

def shutdown_server():
    func = request.environ.get('werkzeug.server.shutdown')
    if func is None:
        logging.warn(f'shutdown_server: Not running with the Werkzeug Server. Please Ctrl-C to quit. Your tokens have been saved.')
    else:
        func()

def store_tokens(access_token, refresh_token):
    with open(expanduser("~/.alexa/tokens.json", "w")) as f:
        f.write(json.dumps({ 'access_token': access_token, 'refresh_token': refresh_token }))

@FlaskApp.route("/auth")
def auth():
    code = request.args.get('code')
    key = request.args.get('key')
    state = request.args.get('state')

    if state != None and key == None:
        key = state

    if code:
        tokens = get_tokens(CONFIG, code)
        if 'error_description' in tokens:
            return tokens['error_description']

        if key == None and state != None:
            key = state

        store_tokens(tokens['access_token'], tokens['refresh_token'])
        logging.debug("auth: Success! Token saved. Shutting down server.")
        shutdown_server()
        return "Success! Your tokens have been saved to your local machine. This server is shutting down."

    if key != None:
        url = "https://www.amazon.com/ap/oa?client_id="+CONFIG["client_id"] \
            + "&scope=alexa%3Aall&scope_data=%7B%22alexa%3Aall%22%3A%7B%22productID%22%3A%22" \
            + CONFIG["program"]+"%22%2C%22productInstanceAttributes%22%3A%7B%22deviceSerialNumber%22%3A%22" \
            + key+"%22%7D%7D%7D&response_type=code&state=" \
            + key+"&redirect_uri=http%3A%2F%2F" \
            + request.host + "/auth"
        logging.debug(f"auth: Redirecting to {url}")
        return redirect(url)

    return "No KEY parameter passed. \nNo local access_token.\nTry visiting the root site."


class Requestor(AlexaClient):
    def __init__(self, config, cwd, verbose=False):
        self.config = config
        self.verbose = verbose
        self.cwd = cwd

        cid = config["client_id"]
        sec = config["secret"]
        ref = read_refresh_token()

        super(Requestor, self).__init__(cid, sec, ref)
        self.ping_manager = self.ping_manager_class(0.5, self.ping) # reset timer
        self.connect()

    def send(self, audio):
        #logging.debug(f"requestor.send: requesting '{audio}'")
        audios = []

        with open(audio, 'rb') as f:
            for i, directive in enumerate(self.send_audio_file(f)):
                if directive.name in ['Speak', 'Play']:
                    #logging.debug(f"Requestor.send: processing {i} {directive.name} {directive}")
                    ofp = os.path.join(self.cwd, 'output.mp3')
                    with open(ofp, 'wb') as f2:
                        #logging.debug(f"Requestor.send: saving to {ofp}")
                        f2.write(directive.audio_attachment)
                        audios.append(ofp)

        return audios
        
def load_config():
    """ Loads the configuration from the current director or home directory"""
    p = "./config.json"
    if not os.path.exists(p):
        p = expanduser("~/.alexa/config.json")

    if not os.path.exists(p):
        raise Exception(f"Cannot find config.json {p}")

    with open(p) as f:
        config = json.load(f)

    return config

def get_cwd(output_dir):
    """ Creates a temporary directory """
    if output_dir is not None:
        if not os.path.exists(output_dir):
            os.mkdir(output_dir)

        return output_dir

    output_dir = f"/tmp/{uuid.uuid4()}"
    os.mkdir(output_dir)
    return output_dir

def unlink_dir(output_dir):
    if output_dir.startswith("/tmp") == False:
        logging.error(f"unlink_dir: I will not delete {output_dir}, I will not, I will not.")
        return

    shutil.rmtree(output_dir)

def tts(text, cwd):
    """ Text to speech for sending requests """
    ifp = os.path.join(cwd, "request.txt")
    ofp = os.path.join(cwd, "request.wav")

    with open(ifp, 'w') as f:
        f.write(text)

    logging.debug(f"tts: calling text2wave -o {ofp} {ifp}")

    # sythesize
    subprocess.call(["text2wave", "-F", "16000", "-o", ofp, ifp])
    
    return ofp

def convert_mp3s_to_wavs(mp3s):
    wavs = []
    for i, mp3 in enumerate(mp3s):
        parts = os.path.splitext(mp3)
        if parts[1].lower() == ".wav":
            continue

        wav = os.path.splitext(mp3)[0] + ".wav"
        logging.debug(f"convert_mp3_to_wav: converting {mp3} to {wav}")
        ffmpeg.input(mp3).output(wav, codec='pcm_u8', ar='16000').run(capture_stdout=True, capture_stderr=True)
        wavs.append(wav)

    return wavs

def stt(audios):
    """ Speech to Text for an array of audio files"""
    response = ""

    r = sr.Recognizer()

    for i, audio in enumerate(audios):
        logging.debug(f"stt: transcribing {i} {audio}")
        with sr.AudioFile(audio) as source:
            a = r.record(source)
            t = r.recognize_sphinx(a)
            logging.debug(f"stt: received '{t}'")
            response = response + " " + t

    response = response.strip()

    return response

def main(text=None, configure=False, verbose=False, output=None):
    """ Alexa CLI is a command-line Alexa client. It requires an AVS account setup. See: https://github.com/xeb/alexa-cli for more info. """
    global CONFIG
    if verbose:
        logging.set_verbosity(logging.DEBUG)

    logging.debug(f"main: text=={text}")
    logging.debug(f"main: configure=={configure}")
    logging.debug(f"main: verbose=={verbose}")
    logging.debug(f"main: output=={output}")
    
    response = None

    if text is None and configure is False:
        logging.error("ERROR. No text specified and not in configuration mode")
        sys.exit(1)

    config = CONFIG
    logging.debug(f"main: Loaded config: {config}")
    
    if configure:
        logging.info(f"main: Starting Flask server for getting tokens")
        server = Server(config)
        server.run()
        return

    cwd = get_cwd(output)

    requestor = Requestor(config, cwd, verbose)
    req_audio = tts(text, cwd)

    audios = requestor.send(req_audio)
    audios = convert_mp3s_to_wavs(audios)
    response = stt(audios)

    # Delete tmp directory
    if output is None:
        unlink_dir(cwd)

    return response

def entry():
    logging.set_verbosity(logging.WARNING)
    fire.Fire(main)

if __name__ == "__main__":
    entry()
