# Alexa CLI

A command line interface for Alexa. 

The goal of this project is to create both a single-file standalone Python script as well as a pip package to communicate with Alexa via the command line.

# Examples
```
$ alexa "turn off the bedroom light"
okay

$ alexa "play all along the watch tower on kitchen"
all along the watch tower by the jimmy hendric's experience playing on kitchen

$ alexa -tg "what is the price of bitcoin" # forces transcribe via Google
one Bitcoin is worth $8,250 up less than 1% over the last 24 hours

$ alexa "how many days until Christmas"
there are 220 days until Christmas Day
```


# Getting Started

## Dependencies
There are many dependencies you will need. 

For Linux (Debian/Ubuntu) try:
```
sudo apt install -y festival sox curl ffmpeg
```

Check to ensure the following commands are working and executable from your shell:

- `sox`
- `text2wave` (Festival)
- `ffmpeg` (ffmpeg)
- `curl`

### Notes to Mac Users
Festival (which contains text2wave) can be found at [github.com/pettarin/setup-festival-mbrola](https://github.com/pettarin/setup-festival-mbrola)

## Transcription Dependency
You can choose one of two transcription services. Google Cloud or Mozilla's Deepspeech. Google may cost you some money but is generally better. Deepspeech runs slower (depending on your GPU) but runs locally. 

See the section below for specific instructions.

### If using Google
Be sure the following command works:
- `gcloud`

See: https://cloud.google.com/sdk/docs/downloads-apt-get

### If using Deepspeech
Be sure the following command works:
- `deepspeech`

See sections below for more information

## Installation
Clone this repository 
```
git clone https://github.com/xeb/alexa-cli
```

Run the install via:
```
make install
```

Verify the tool installed correctly by just running:
```
alexa
```
which should display the help menu

## Configuration
You will need to setup an AVS (Alexa Voice Service) developer account as well as link your Amazon account.

### Start the Configuration Process
Run: 
```
alexa --configure
```

### AVS Account
1) Visit [developer.amazon.com/avs/home.html#/avs/home](https://developer.amazon.com/avs/home.html#/avs/home)
2) Create a new Product
3) Enter your Client ID, Client Secret and Program ID into the command line

IMPORTANT: you need to register the OAuth URLs to authenticate with your Amazon account tokens. If you are only using this tool locally, just add localhost. If you are setting it up remotely, you will need to add a remote endpoint that can access the server you are running the script on. This is only required for setup.

Under "Security Profile" in the AVS Console add:

- Allowed origin of `http://localhost:8086`
- Allowed return URLs of `http://localhost:8089/auth`


### Save Transcription
You will be prompted if you want to save transcriptions. This is helpful. A small database will be kept in `~/.alexa/transcriptions.db` to avoid re-transcribing the same audio responses from Alexa. But you don't have to do this.

### Transcription Type
Decide if you would like to use Deepspeech or not. If you do decide to use Deepspeech, you will have to enter the path to the models. 

NOTE: the tool supports changing transcription type at runtime. Use the flag `-tg` to force Google transcriptions or `-td` to force Deepspeech transcriptions. I find it helpful to alternate depending on the command. I usually have Deepspeech for default and Google as needed.

See below for more information

### Initialize Tokens
You will be prompted if you want to initialize tokens, say "y".

The CLI will start a Flask server & then launch a browser for you to authenticate with the Amazon credentials that will connect Alexa Voice Service.

You can redo this process at any point via 
```
alexa --tokens
```

NOTE: you can also specify token keys to register more than one account. See examples below for more info.

## Try it out!
```
alexa -v "what time is it"
main: Verbose mode enabled
main: Copying artifacts to None
get_access_token: using key 'blablah'
...
transcribe_save: saving hash '0f531119ee4877767756792dad271410' with 32 characters
{'success': True, 'result': 'the time is eleven seventeen p m'}
```

# Transcription Instructions

## Google Cloud Transcriptions
Go through the [gcloud setup](https://cloud.google.com/sdk/) & put the credentials file that gets generated here: `~/.gcloud/gcloud-alexa-cli.json`.  Once that is done, you should be able to run something like:

```
export GOOGLE_APPLICATION_CREDENTIALS=~/.gcloud/gcloud-alexa-cli.json
export GOOGLE_ACCESS_TOKEN=`gcloud auth application-default print-access-token`
echo $GOOGLE_ACCESS_TOKEN
```

## Mozilla DeepSpeech
If you'd like to use Mozilla's DeepSpeech, following the instructions at [github.com/mozilla/deepspeech](https://github.com/mozilla/deepspeech).

Be sure to also download the [pretrained models](https://github.com/mozilla/DeepSpeech/releases) & make a note of the path as that will be used during Configuration.

I would encourage you to install `deepspeech-gpu` for faster results.

# Docker 
I'm not done with a containerized version of the Alexa CLI for Docker. My end plan is to have the tool equally distributable (without personalized keys) as a single container, but not quite there yet. You can build the container like so:
```
make container
```

If you want to jump into a shell with everything loaded, try:
```
make debug
```

# More Examples

### Helpful calculations
```
$ alexa "how many minutes is three thousand six hundred seconds"
3600 seconds is 60 minutes

$ alexa "how many days is 1,515 hours"
1515 hours is 63.13 days

```

### How to reconfigure or update a value
```
$ alexa --configure # Setup AVS Client ID, Secret, and Program Name

$ alexa --tokens # Setup your Amazon account credentials

```

### If you use different Amazon accounts, you can save tokens and use different keys as needed (e.g. work or personal accounts)
```
$ alexa --tokens --token_key="personal" # Setup your Amazon account credentials for a specific key

$ alexa --list_tokens # List all available tokens

$ alexa --token_key="personal" "play all along the watch tower on office"
all along the watch tower by the jimmy hendrix experience playing on home office

$ alexa --token_key="work" "turn off desk light"
ok
```

### Keep all output and artifacts in a folder 
```
$ alexa --verbose --artifacts --output="./artifacts/request" "what time is it"

```

# Words of Warning
This project is very much a giant hack. But I find this interface useful and am happy to share with others.

# Issues
Please submit issues here on GitHub and I'll do my best to address