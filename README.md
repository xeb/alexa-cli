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
echo "{\"client_id\":\"{YOUR_CLIENT_ID}\",\"secret\":\"{SECRET}\",\"program\":\"{PROGRAM}\"}" > ~/.alexa/config.jaon
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



## Part Four - Test it out
