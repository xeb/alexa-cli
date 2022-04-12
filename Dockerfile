FROM python:3.8
RUN apt-get update -y && apt-get install -y swig build-essential libasound2-dev libpulse-dev festival ffmpeg
RUN mkdir -p /alexa-cli
COPY VERSION requirements.txt Makefile setup.py setup.cfg /alexa-cli/
COPY alexa_cli /alexa-cli/alexa_cli
RUN cd /alexa-cli/ && make
