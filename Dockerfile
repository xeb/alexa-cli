FROM python:3.6.5-jessie
RUN apt-get update -y
RUN apt-get update -y && apt-get install -y libav-tools python-pip festival sox curl
RUN pip install requests Flask pytest deepspeech
RUN mkdir -p /usr/local/alexa/alexatext
WORKDIR /usr/local/alexa
ADD *.py /usr/local/alexa/
ADD alexatext/* /usr/local/alexa/alexatext/
ADD tests /usr/local/alexa/
ADD requirements.txt /usr/local/alexa/
ADD VERSION /usr/local/alexa/
ADD Makefile /usr/local/alexa/
RUN make install

CMD [  ]