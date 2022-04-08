#!/usr/bin/env python
"""
Alexa CLI
-------------
A hacky but small script that acts as an Alexa Voice Service client allowing command-line access to Alexa.

Usage:
---
$ python cli.py "what time is it"
its nine fifteen p m

$ python cli.py -h

$ python cli.py --configure # Setup AVS Client ID, Secret, and Program Name

$ ./cli.py --token_key="personal" --verbose --artifacts --output="./artifacts/request" "what time is it"
...

-------------
"""
import os
from os.path import expanduser
import sys
import time
import json
import uuid
import re
import stat
import shutil
import hashlib
import base64
import webbrowser
import socket
import argparse
import subprocess
from distutils.spawn import find_executable
import sqlite3
import requests
from flask import Flask, request, redirect, send_from_directory, has_request_context

PORT = 8086
TOKEN_KEY = None
VERBOSE_MODE = False
TOKENS_DB = expanduser("~/.alexa/tokens.db")
TRANSCRIPTIONS_DB = expanduser("~/.alexa/transcriptions.db")
CONFIG_PATH = expanduser('~/.alexa/config.json')
REQUIRED_TOOLS = ["text2wave", "sox", "ffmpeg", "curl"]
Application = Flask(__name__)


def log(msg, *params):
    """ Simple logging function that checks global verbosity """
    global VERBOSE_MODE
    if VERBOSE_MODE and len(params) > 0:
        print(msg % params)


def get_token_key():
    """ gets the token key if defined or defaults to hostname """
    global TOKEN_KEY
    if TOKEN_KEY is None:
        return socket.gethostname()
    return TOKEN_KEY


@Application.route("/")
def welcome():
    """ Root request which redirects """
    return redirect("http://" + request.host + "/auth?state=" + get_token_key())


@Application.route("/auth")
def auth():
    """ Authenticates for the given AVS client """
    code = request.args.get('code')
    key = request.args.get('key')
    state = request.args.get('state')

    if state != None and key == None:
        key = state

    if code:
        tokens = get_tokens(code)
        if 'error_description' in tokens:
            return tokens['error_description']

        if key == None and state != None:
            key = state

        store_tokens(key, tokens['access_token'], tokens['refresh_token'])
        print("auth: Success! Token saved. Shutting down server.")
        shutdown_server()
        return "Success! Your tokens have been saved to your local machine. This server is shutting down."

    if key != None:
        url = "https://www.amazon.com/ap/oa?client_id="+CONFIG["clientId"] \
            + "&scope=alexa%3Aall&scope_data=%7B%22alexa%3Aall%22%3A%7B%22productID%22%3A%22" \
            + CONFIG["programId"]+"%22%2C%22productInstanceAttributes%22%3A%7B%22deviceSerialNumber%22%3A%22" \
            + key+"%22%7D%7D%7D&response_type=code&state=" \
            + key+"&redirect_uri=http%3A%2F%2F" \
            + request.host + "/auth"
        log("auth: Redirecting to %s", url)
        return redirect(url)

    return "No KEY parameter passed. \nNo local access_token.\nTry visiting the root site."


@Application.route("/access_token")
def access_token(key=None, force_refresh=False):
    """ Gets the current access_token """
    log("access_token: using key=%s, force_refresh=%s", key, force_refresh)
    if key == None and has_request_context():
        key = request.args.get("key")
    if key is None or key == "":
        key = get_token_key()
    return get_access_token(key, force_refresh)


def get_access_token(key, force_refresh=False):
    """ gets a given access token """
    sql_conn = sqlite3.connect(TOKENS_DB)
    create_table(sql_conn)
    cursor = sql_conn.cursor()
    log("get_access_token: using key '%s'" % key)
    cursor.execute(
        "SELECT access_token, refresh_token, last_accessed FROM users WHERE key = '{0}';".format(key))
    rows = cursor.fetchall()
    sql_conn.close()
    if len(rows) != 1:
        return None

    epoch = int(time.mktime(time.strptime(rows[0][2], "%Y-%m-%d %H:%M:%S")))
    diff = int(time.time()) - epoch
    log("get_access_token: Found epoch of %s vs %s and diff of %s",
        epoch, time.time(), diff)
    if diff >= 3600 or force_refresh:
        log("get_access_token: Refreshing token")
        tokens = refresh_tokens(rows[0][1])
        if 'error_description' in tokens:
            log("Error: %s" % tokens['error_description'])
            return None

        store_tokens(key, tokens["access_token"], tokens["refresh_token"])
        return tokens["access_token"]

    return rows[0][0]


def list_access_tokens():
    """ Lists access tokens that are registered in the user's personal DB """
    sql_conn = sqlite3.connect(TOKENS_DB)
    create_table(sql_conn)
    cursor = sql_conn.cursor()
    log("list_access_tokens: getting all keys")
    cursor.execute("SELECT key FROM users;")
    rows = cursor.fetchall()
    sql_conn.close()

    print("Found keys:%s" % rows)
    i = 0
    for row in rows:
        print("\tKey[%s] == '%s'" % (i, row[0]))
        i = i + 1

    print("Usage:\n$ alexa -k \"{KEY}\" \"what time is it\" ")
    print("$ alexa --token_key=\"{KEY}\" \"what time is it\" ")


def get_redirect_uri():
    """ Redirects the default URI """
    redirect_uri = None
    if has_request_context():
        redirect_uri = "http://" + request.host + "/auth"
    else:
        redirect_uri = 'http://localhost:%s/auth' % PORT
    return redirect_uri


def get_tokens(code):
    redirect_uri = get_redirect_uri()
    payload = {'grant_type': 'authorization_code', 'code': code,
               'client_id': CONFIG["clientId"], 'client_secret': CONFIG["clientSecret"], 'redirect_uri': redirect_uri}
    log("get_tokens: Getting Authorization Code with payload %s", payload)
    url = "https://api.amazon.com/auth/o2/token"
    r = requests.post(url, data=payload)
    tokens = r.json()
    return tokens


def refresh_tokens(refresh_token):
    redirect_uri = get_redirect_uri()
    payload = {'grant_type': 'refresh_token', 'refresh_token': refresh_token,
               'client_id': CONFIG["clientId"], 'client_secret': CONFIG["clientSecret"], 'redirect_uri': redirect_uri}
    log("refresh_tokens: Refreshing Token with payload")
    url = "https://api.amazon.com/auth/o2/token"
    r = requests.post(url, data=payload)
    tokens = r.json()
    log("refresh_tokens: Received refresh tokens %s", tokens)
    return tokens


def shutdown_server():
    func = request.environ.get('werkzeug.server.shutdown')
    if func is None:
        raise RuntimeError('Not running with the Werkzeug Server')
    func()


def create_table(sql_conn):
    sql = '''CREATE TABLE IF NOT EXISTS users
    (key INT PRIMARY KEY NOT NULL,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    last_accessed DATETIME NOT NULL);'''
    sql_conn.execute(sql)
    sql_conn.commit()

def create_table_transcription(sql_conn):
    log("create_table_transcription: creating table if not exists...")
    sql = '''
    CREATE TABLE IF NOT EXISTS transcriptions
    (hash TEXT PRIMARY KEY NOT NULL,
    transcription TEXT NOT NULL,
    last_accessed DATETIME NOT NULL);'''
    sql_conn.execute(sql)
    sql_conn.commit()
    log("create_table_transcription: success")

def store_tokens(key, access_token, refresh_token):
    if key == None:
        print("Cannot save key value of None")
        return

    sql_conn = sqlite3.connect(TOKENS_DB)
    create_table(sql_conn)
    sql = '''INSERT OR IGNORE INTO users VALUES
           ('{0}', '{1}', '{2}', CURRENT_TIMESTAMP);'''.format(key, access_token, refresh_token)
    sql_conn.execute(sql)
    sql = '''UPDATE users SET
                access_token = '{1}',
                refresh_token = '{2}',
                last_accessed = CURRENT_TIMESTAMP
                WHERE key = '{0}';'''.format(key, access_token, refresh_token)
    sql_conn.execute(sql)
    sql_conn.commit()


def validate_tools():
    """ Quick validation to ensure tools are setup correctly """
    for audio_exec in REQUIRED_TOOLS:
        if find_executable(audio_exec) == None:
            print("Cannot find %s in path. Please install it. See: github.com/xeb/alexa-cli for more details. " % (audio_exec))
            sys.exit(1)


def init_tokens():
    """ Starts the server to initialize OAuth token exchange """
    log("init_tokens: Running initialization to get Alexa tokens...")
    webbrowser.open('http://localhost:%s/' % PORT)
    Application.run(host="0.0.0.0", port=PORT)

def config_prompt_setting(output_name, config_key, is_mask=False, is_bool=False):
    """ Helper method to prompt for a configuration setting """
    display = "" if not is_bool else " (Y/N)"
    if config_key in CONFIG:
        if is_mask:
            display = " (currently: %s****%s)" % (CONFIG[config_key][:6],CONFIG[config_key ][-4:])
        elif is_bool:
            display = " (currently: %s)" % ("Y" if CONFIG[config_key] else "N")
        else:
            display = " (currently: %s)" % CONFIG[config_key]

    sys.stdout.write("{0}{1}: ".format(output_name, display))
    sys.stdout.flush()
    val = sys.stdin.readline().strip()
    if val is None or val == "":
        if config_key not in CONFIG:
            CONFIG[config_key] = ""

        val = str(CONFIG[config_key])

    if is_bool:
        return val.lower() == "y" or val.lower() == "true"

    return val

def init_config(reconfig=False, force_use_deepspeech=None):
    """ Initializes the configuration file stored in ~/.alexa/config.json """
    global CONFIG
    CONFIG = {}
    if os.path.exists(CONFIG_PATH):
        CONFIG = json.load(open(CONFIG_PATH))
        if force_use_deepspeech is not None:
            log("init_config: forcing transcription to: use_deepspeech==%s", force_use_deepspeech)
            CONFIG["useDeepspeech"] = force_use_deepspeech
            CONFIG["forceTranscribe"] = True # HACK: shouldn't have to do this

        if not reconfig:
            return

    print("No configuration found. Let's setup your Client ID, Client Secret and Program. \nSee: github.com/xeb/alexa-cli for more details.\nHit ENTER to keep current value\n")

    client_id = config_prompt_setting("Client ID", "clientId", is_mask=True, is_bool=False)
    client_secret = config_prompt_setting("Client Secret", "clientSecret", is_mask=True, is_bool=False)
    program_id = config_prompt_setting("Program ID", "programId", is_mask=False, is_bool=False)
    save_transcription = config_prompt_setting("Save Transcription", "saveTranscription", is_mask=False, is_bool=True)
    use_deepspeech = config_prompt_setting("Use Deepspeech", "useDeepspeech", is_mask=False, is_bool=True)
    model_path = ""
    if use_deepspeech:
        model_path = config_prompt_setting("Deepspeech Model Path", "deepspeechModelPath", is_mask=False, is_bool=False)

    CONFIG["clientId"] = client_id
    CONFIG["clientSecret"] = client_secret
    CONFIG["programId"] = program_id
    CONFIG["saveTranscription"] = save_transcription
    CONFIG["useDeepspeech"] = force_use_deepspeech if force_use_deepspeech is not None else use_deepspeech

    if model_path != "":
        CONFIG["deepspeechModelPath"] = expanduser(model_path)

    config_dir = os.path.dirname(CONFIG_PATH)
    if not os.path.exists(config_dir):
        os.mkdir(config_dir)

    json.dump(CONFIG, open(CONFIG_PATH, 'w'))
    print("Configuration saved.")
    sys.stdout.write("Would you like to initialize tokens? [Y/n] ")
    sys.stdout.flush()
    ouput = sys.stdin.readline().strip()
    if ouput.lower() in ["y", "yes"]:
        init_tokens()


def split_by_marker(f, marker="", block_size=4096):
    current = b''
    while True:
        block = f.read(block_size)
        if not block:  # end-of-file
            yield current
            return
        current += block
        while True:
            markerpos = current.find(marker.encode())
            if markerpos < 0:
                break
            yield current[:markerpos]
            current = current[markerpos + len(marker):]


def post_process_response(tmp_dir):
    def tmp(path): return os.path.join(tmp_dir, path)
    log("post_process_response: Parsing %s", tmp("alexa_response.result"))
    boundary = None
    with open(tmp("alexa_response.result"), 'rb') as result_file:
        contents = result_file.read()
        if "HTTP/1.1 200 OK".encode() in contents:
            log("post_process_response: Successful response found")
        else:
            log("post_process_response: ERROR: No '200 OK' received")
            # HERE
            return False

        log("post_process_response: Received %s bytes" % len(contents))

        log("post_process_response: Finding boundary")
        content_lines = contents.split(b"\n")
        d = str(content_lines[5:6][0])
        log("post_process_response: Searching %s", d)
        m = re.search("boundary=(.*?);", d)
        if m == None:
            log("post_process_response: No match found for boundary")
            sys.exit(1)

        boundary = m.group(1)

    log("post_process_response: Using boundary %s", boundary)
    items = list(split_by_marker(
        open(tmp("alexa_response.result"), 'rb'), boundary))
    log("post_process_response: found %s items in response" % len(items))

    for i in range(2, len(items)):
        if i >= len(items):
            break
        with open(tmp('part-%s.mp3' % i), 'wb') as fw:
            fw.write(items[i])

        log("post_process_response: Wrote %s" % tmp("part-%s.mp3" % i))

    # Let's just make a script that can pull all this audio together and convert with ffmpeg
    script = "#!/bin/bash\n \
f=\"result.mp3\"\n \
find *.mp3 -type f -size +4096c -exec cat {} \\; > $f \n \
ffmpeg -i \"$f\" -acodec pcm_s16le -ac 1 -ar 16000 \"${f%.mp3}.wav\" > /dev/null \n \
ffmpeg -i result.wav -af aformat=s16:16000 result.flac \n \
        "
    with open(tmp("convert.sh"), 'w') as fw:
        fw.write(script)

    # feeling dirty...
    st = os.stat(tmp("convert.sh"))
    os.chmod(tmp("convert.sh"), st.st_mode | stat.S_IEXEC)
    p = subprocess.Popen("./convert.sh", shell=True,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=tmp_dir)
    stdout, stderr = p.communicate()
    log("post_process_response: executed concat and convert script, result: %s", p.returncode)

    if p.returncode != 0:
        log("post_process_response: ERROR with convert! %s", stderr)
        return False

    # Convert the FLAC file to base64 to upload to Google
    with open(tmp("result.flac"), "rb") as flac:
        encoded_string = base64.b64encode(flac.read())
        with open(tmp("result.base64"), "wb") as b64:
            b64.write(encoded_string)

    log("post_process_response: created base64 file of FLAC")
    return True

def transcribe_get(hash):
    sql_conn = sqlite3.connect(TRANSCRIPTIONS_DB)
    create_table_transcription(sql_conn)
    cursor = sql_conn.cursor()
    log("transcribe_get: using hash '%s'" % hash)
    cursor.execute(
        "SELECT transcription FROM transcriptions WHERE hash = '{0}';".format(hash))
    rows = cursor.fetchall()
    sql_conn.close()
    return rows[0][0] if len(rows) > 0 else None


def transcribe_save(hash, transcription):
    sql_conn = sqlite3.connect(TRANSCRIPTIONS_DB)
    create_table_transcription(sql_conn)
    cursor = sql_conn.cursor()
    log("transcribe_save: saving hash '%s' with %s characters" % (hash, len(transcription)))
    sql = "INSERT INTO transcriptions VALUES ('{0}','{1}',CURRENT_TIMESTAMP);".format(hash, transcription.replace("'","''"))
    sql_conn.execute(sql)
    sql_conn.commit()
    sql_conn.close()


def transcribe_file(tmp_dir):
    save_transcription = "saveTranscription" in CONFIG and CONFIG["saveTranscription"]
    if "forceTranscribe" in CONFIG and CONFIG["forceTranscribe"]:
        log("transcribe_file: forcing to not load or save the transcription")
        save_transcription = False
    
    log("transcribe_file: save_transcription is %s", save_transcription)

    result_hash = None

    # Avoid hashing the file unless we are saving the result
    if save_transcription:
        full_path = os.path.join(tmp_dir, "result.wav")
        result_hash = hashlib.md5(open(full_path, 'rb').read()).hexdigest()
        log("transcribe: result_hash == %s", result_hash)
        result = transcribe_get(result_hash)
        if result != None:
            return result

    if CONFIG["useDeepspeech"]:
        result = transcribe_with_deepspeech(tmp_dir)
    else:
        result = transcribe_from_google(tmp_dir)

    if save_transcription:
        transcribe_save(result_hash, result)

    return result


def transcribe_with_deepspeech(tmp_dir):
    """
    Transcribes assets in given tmp directory into text assets via Mozilla's DeepSpeech
    """
    def tmp(path): return os.path.join(tmp_dir, path)

    deepspeech_model_path = CONFIG["deepspeechModelPath"]
    cmds = ["deepspeech", 
            "--model", os.path.join(deepspeech_model_path, "output_graph.pb"),
            "--audio", os.path.join(tmp_dir, "result.wav"),
            # "--alphabet", os.path.join(deepspeech_model_path, "alphabet.txt"),
            # "--trie", os.path.join(deepspeech_model_path, "trie")
            ]
    
    log("transcribe_with_deepspeech: `%s`" % " ".join(cmds))
    log("transcribe_with_deepspeech: transcribing from %s using model %s" %
        (tmp_dir, deepspeech_model_path))
        
    p = subprocess.Popen(" ".join(cmds), shell=True,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=tmp_dir)

    stdout, stderr = p.communicate()

    text = stdout.decode("utf-8").strip()
    if stderr and stderr != "":
        print(f"transcribe_with_deepspeech: stderr=={str(stderr)}")

    log(f"transcribe_with_deepspeech: transcribed \n----\n'{text}'\n")
    return text


def transcribe_from_google(tmp_dir):
    """
    Transcribes assets in given tmp directory into text assets via Google Cloud Transcribe
    """
    def tmp(path): return os.path.join(tmp_dir, path)
    script = "#!/bin/bash\n \
export GOOGLE_APPLICATION_CREDENTIALS=~/.gcloud/gcloud-alexa-cli.json \n \
export ACCESS_TOKEN=`gcloud auth application-default print-access-token` \n \
echo $ACCESS_TOKEN \n \
    "
    with open(tmp("google-token.sh"), 'w') as fw:
        fw.write(script)

    # feeling dirty...
    st = os.stat(tmp("google-token.sh"))
    os.chmod(tmp("google-token.sh"), st.st_mode | stat.S_IEXEC)
    p = subprocess.Popen("./google-token.sh", shell=True,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=tmp_dir)
    stdout, stderr = p.communicate()
    log("transcribe_from_google: executed google-token script, result: %s", p.returncode)

    if p.returncode != 0:
        log("transcribe_from_google: ERROR with google-token! %s", stderr)
        return

    token = str(stdout.strip())
    token = token[2:len(token)-1]
    log("transcribe_from_google: token is %s", token)

    request_content = """{
    "config": {
        "encoding":"FLAC",
        "sampleRateHertz": 16000,
        "languageCode": "en-US",
        "enableWordTimeOffsets": false
    },
    "audio": {
        "content":"%s" } }""" % (open(tmp("result.base64"), 'r').read())

    with open(tmp("request-transcribe.json"), 'w') as transcribe_write:
        transcribe_write.write(request_content)

    script = """#!/bin/bash
curl -s -H "Content-Type: application/json"\\
    -H "Authorization: Bearer %s"\\
    https://speech.googleapis.com/v1/speech:recognize \\
    -d@request-transcribe.json > transcript-output.json

    """ % (token)

    with open(tmp("google-transcribe.sh"), 'w') as fw:
        fw.write(script)

    # feeling dirty...
    st = os.stat(tmp("google-transcribe.sh"))
    os.chmod(tmp("google-transcribe.sh"), st.st_mode | stat.S_IEXEC)
    p = subprocess.Popen("./google-transcribe.sh", shell=True,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=tmp_dir)
    stdout, stderr = p.communicate()
    log("transcribe_from_google: executed google-transcribe script, result: %s", p.returncode)

    if not os.path.exists(tmp("transcript-output.json")):
        log("transcribe_from_google: Could not find transcript-output.json")
        return

    transcript = json.load(open(tmp("transcript-output.json"), 'r'))
    if transcript == None or "results" not in transcript:
        log("transcribe_from_google: No results from transcription")
        return

    log("transcribe_from_google: returning transcript text from %s", transcript)
    text = transcript["results"][0]["alternatives"][0]["transcript"]
    log("transcribe_from_google: %s", text)
    return text


def request_from_alexa(text_input, keep_artifacts=False, loc_artifacts=None, token_key=None, token_refresh=False, transcribe=True):
    token = access_token(token_key, token_refresh)

    log("request_from_alexa: Requesting: \n----\n'%s'\n----\n", text_input)
    tmp_dir = "/tmp/%s/" % uuid.uuid4()

    def tmp(path): return os.path.join(tmp_dir, path)
    if not os.path.exists(tmp_dir):
        os.mkdir(tmp_dir)

    with open(tmp("request_text.txt"), 'w') as fw:
        fw.write(text_input)

    metadata = {
        "messageHeader": {},
        "messageBody": {
            "profile": "alexa-close-talk",
            "locale": "en-us",
            "format": "audio/L16; rate=16000; channels=1"
        }
    }

    json.dump(metadata, open(tmp("metadata.json"), 'w'))

    # generate initial audio
    cmds = ["text2wave", "-o", tmp("init_audio.wav"), tmp("request_text.txt")]
    subprocess.call(cmds)

    # use sox to process down to 16kHz
    cmds = ["sox", "-", "-c", "1", "-r", "16000", "-e",
            "signed", "-b", "16", tmp("request_audio.wav")]
    ps = subprocess.Popen(('cat', tmp("init_audio.wav")),
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = subprocess.check_output(cmds, stdin=ps.stdout, stderr=ps.stderr)
    ps.wait()

    log("request_from_alexa: sox output: %s", output)

    log("request_from_alexa: audio generated %s", tmp("request_audio.wav"))

    # TODO: replace this request with... requests!
    cmds = ["curl", "-i", "-s", "-H", "Authorization: Bearer {}".format(token),
            "-F", "metadata=<{};type=application/json; charset=UTF-8".format(
                tmp("metadata.json")),
            "-F", "audio=<{};type=audio/L16; rate=16000; channels=1".format(
                tmp("request_audio.wav")),
            "-o", tmp("alexa_response.result"),
            "https://access-alexa-na.amazon.com/v1/avs/speechrecognizer/recognize"
            ]
    log("request_from_alexa: sending request to Alexa...")
    log(" ".join(cmds))
    print(" ".join(cmds))
    out = subprocess.call(cmds)
    log("request_from_alexa: status code from cURL: %s", out)

    if not os.path.exists(tmp("alexa_response.result")):
        print("No responses from AVS")

    halt = False
    result_bool = False
    try:
        result_bool = post_process_response(tmp_dir)
        if result_bool == False:
            halt = True
    except Exception as e:
        log("request_from_alexa: something went wrong!")
        log("request_from_alexa: Could not post_process artifacts: Error: %s", e)
        halt = True

    result = ""
    if halt is False and transcribe:
        # try:
        result = transcribe_file(tmp_dir)
        # except Exception as e:
        #     log("request_from_alexa: Could not transcribe: Error: %s" % e)
        #     halt = True

    if transcribe is False:
        log("request_from_alexa: skipping transcription")

    if loc_artifacts != None:
        output_loc = expanduser(os.path.join(loc_artifacts, str(uuid.uuid4())))
        shutil.copytree(tmp("."), output_loc)
        log(f"request_from_alexa: artifacts available in {output_loc}", output_loc)

    if keep_artifacts == False:
        for file in os.listdir(tmp_dir):
            os.unlink(tmp(file))

        os.rmdir(tmp_dir)
    else:
        log(f"request_from_alexa: temp artifacts available in {tmp_dir}")

    return {'success': result_bool, 'result': result}


def request_from_alexa_retry(text_input=None, keep_artifacts=False, loc_artifacts=None, token_key=None, transcribe=True):
    result = request_from_alexa(text_input=text_input,
                                keep_artifacts=keep_artifacts,
                                loc_artifacts=loc_artifacts,
                                token_key=token_key,
                                transcribe=transcribe)

    if result['success'] is False:
        log("request_from_alexa_retry: Received ERROR, retrying...")
        print("Retrying...")
        result = request_from_alexa(text_input=text_input,
                                    keep_artifacts=keep_artifacts,
                                    loc_artifacts=loc_artifacts,
                                    token_key=token_key,
                                    token_refresh=True,
                                    transcribe=transcribe)

    return result

def main():
    global VERBOSE_MODE
    global TOKEN_KEY
    TOKEN_KEY = None

    parser = argparse.ArgumentParser("alexa")
    parser.add_argument("-c", "--configure", help="Reconfigures your client ID, client secret and programId with AVS. Stored in",
                        required=False, action='store_true')
    parser.add_argument("-t", "--tokens", help="Initialize your access tokens",
                        required=False, action='store_true')
    parser.add_argument("-tg", "--transcribe_with_google", help="Force Google transcription",
                        required=False, action='store_true')
    parser.add_argument("-td", "--transcribe_with_deepspeech", help="Force Deepspeech transcription",
                        required=False, action='store_true')
    parser.add_argument("-v", "--verbose", help="Output verbose options",
                        required=False, action='store_true')
    parser.add_argument("-a", "--artifacts", help="Indicates whether or not to keep the generated artifacts",
                        required=False, action='store_true')
    parser.add_argument(
        "-o", "--output_dir", help="The output directory for all articacts (AVS response, transformed audio, etc.)", required=False)
    parser.add_argument("-k", "--token_key",
                        help="The token key to use for storage", required=False)
    parser.add_argument("-lk", "--list_token_keys",
                        help="List all available token keys", required=False, action='store_true')

    parser.add_argument(
        "text_input", help="Your request to Alexa enclosed in quotes. Example: \"what time is it\"", nargs='?', default='')
    args = parser.parse_args()

    force_transcription_source = False
    use_deepspeech = False
    if args.transcribe_with_google:
        log("main: forcing transcription with Google")
        force_transcription_source = True
        use_deepspeech = False
    elif args.transcribe_with_deepspeech:
        log("main: forcing transcription with Deepspeech")
        force_transcription_source = True
        use_deepspeech = True

    if args.verbose:
        VERBOSE_MODE = True
        log("main: Verbose mode enabled")

    validate_tools()

    if args.token_key:
        TOKEN_KEY = args.token_key
        log("main: using TOKEN_KEY of %s", TOKEN_KEY)

    init_config(args.configure, force_use_deepspeech=use_deepspeech if force_transcription_source else None)

    if args.artifacts:
        log("main: Keeping temp artifacts")

    if args.tokens:
        init_tokens()
        sys.exit(0)

    if args.list_token_keys:
        list_access_tokens()
        sys.exit(0)

    if VERBOSE_MODE:
        log("main: Copying artifacts to %s", args.output_dir)

    if not args.tokens and args.text_input is not None and args.text_input != "":

        if args.output_dir is not None and len(args.output_dir) > 0:
            args.artifacts = True

        result = request_from_alexa_retry(text_input=args.text_input,
                                          keep_artifacts=args.artifacts,
                                          loc_artifacts=args.output_dir,
                                          token_key=TOKEN_KEY)

        if result['success'] is False:
            print("FAILURE: no response received")

        if VERBOSE_MODE:
            print(result)
        else:
            print(result['result'])

    elif not args.configure:
        parser.print_help()

if __name__ == "__main__":
    main()
