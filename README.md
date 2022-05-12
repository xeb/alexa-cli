# Alexa CLI



A command line interface for Alexa. This projecg uses voice synthesis and speech recognition to create a text-based CLI for issuing requests to Alexa.



# Examples

```

$ alexa "turn off the bedroom light"

okay



$ alexa "what time is it?" --verbose

it's two twelve p. m.



$ alexa --help

# ... 

FLAGS

    --text=TEXT

        Type: Optional[]

        Default: None

    --configure=CONFIGURE

        Default: False

    --verbose=VERBOSE

        Default: False

    --output=OUTPUT

        Type: Optional[]

        Default: None

```



# Installation



## Part One - Setup AVS Account



1) Visit [developer.amazon.com/avs/home.html#/avs/home](https://developer.amazon.com/avs/home.html#/avs/home)

2) Create a new Product

3) Enter your Client ID, Client Secret and Program ID into the command line



IMPORTANT: you need to register the OAuth URLs to authenticate with your Amazon account tokens. If you are only using this tool locally, just add localhost. If you are setting it up remotely, you will need to add a remote endpoint that can access the server you are running the script on. This is only required for setup.



Under "Security Profile" in the AVS Console add:



- Allowed origin of `http://localhost:8086`

- Allowed return URLs of `http://localhost:8086/auth`



Now that you have a `Client ID`, `Secret`, and `Program ID` create a JSON file either in the current directory or at `~/.alexa/config.json` for example:



```

mkdir -p ~/.alexa

echo "{\"client_id\":\"{YOUR_CLIENT_ID}\",\"secret\":\"{SECRET}\",\"program\":\"{PROGRAM}\"}" > ~/.alexa/config.json

```



Now that you have a `config.json` in your home directory, you can setup the runtime



## Part Two - Install Runtime



Install deendencies, clone this repo, create a virtualenv, install requirements, and make sure the runtime works. See below for Docker usage.



First, below are dependencies required as part of the speech synthesis and voice recognition phases of the CLI. I'm focused on Linux but Festival and PocketSphinx should work on macOS and Windows. Or you can look at the Docker approach.

```

sudo apt-get update -y

sudo apt-get install -y swig build-essential libasound2-dev libpulse-dev festival ffmpeg

```



Next, let's create a Virtual Environment

```

python -m virtualenv venv

. venv/bin/activate

```



Last, just run `make`. Feel free to look at what it's doing in the `Makefile` but this will install the runtime and requirements.txt.

```

make

```



## Part Three - Configuration and Registration

Now, create a JSON file called `config.json` and put it in the current directory, or in `~/.alexa/config.json`. This configuration should have the client_id, secret, and program name for the application you setup in Part One.

```

mkdir -p ~/.alexa/

echo "{\"client_id\":\"{YOUR_CLIENT_ID}\",\"secret\":\"{YOUR_SECRET}\",\"program\":\"{YOUR_PROGRAM}\"}" > ~/.alexa/config.json

```



Now, you need to associate the client with your Amazon account using the AVS credentials above. Assuming `make` worked correctly (see Part Two), then run:



```

alexa --configure

```



This will start a webserver and show you the URL to visit. Go to: `http://localhost:8086/` (or whatever your IP is). Login, and the client will save your refresh token to `~/.alexa/tokens.json`.



## Part Four - Test it out

You can now try a command abd validate everything is working end-to-end.



```

$ alexa "what time is it" --verbose
DEBUG:absl:main: text==what time is it
DEBUG:absl:main: configure==False
DEBUG:absl:main: verbose==True
DEBUG:absl:main: output==None
DEBUG:absl:main: Loaded config: {'client_id': 'amzn1.application-oa2-client.xxxxxxxx', 'secret': 'xxxxxxxxxx', 'program': 'AlexaCLI'}
DEBUG:absl:tts: calling text2wave -o /tmp/ad3e82dc-e5e8-4cc9-a3cd-4e37f306575b/request.wav /tmp/ad3e82dc-e5e8-4cc9-a3cd-4e37f306575b/request.txt
DEBUG:absl:convert_mp3_to_wav: converting /tmp/ad3e82dc-e5e8-4cc9-a3cd-4e37f306575b/output.mp3 to /tmp/ad3e82dc-e5e8-4cc9-a3cd-4e37f306575b/output.wav
DEBUG:absl:stt: transcribing 0 /tmp/ad3e82dc-e5e8-4cc9-a3cd-4e37f306575b/output.wav
DEBUG:absl:stt: received 'it's eleven am'
it's eleven am
```

