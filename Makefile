.PHONY: time

install: clean
	python -m pip install -r requirements.txt
	python setup.py install --force

test: clean
	pytest .

coverage: clean
	pytest --cov="alexatext" .

# TODO: copy models into the BUILD so they get distributed
container:
	-mkdir -p ./.alexa
	cp -r `python -c 'import json; from os.path import expanduser; CONFIG_PATH=expanduser("~/.alexa/config.json"); print(json.load(open(CONFIG_PATH))["deepspeechModelPath"])'` ./.alexa/
	docker build -t alexacli/alexacli -f Dockerfile .

publish:
	docker push alexacli/alexacli 

# Update the config file so the model inside the container is used at runtime
# TODO: get gcloud working at runtime
debug: container
	-mkdir -p ./.alexa
	cp -f ~/.alexa/config.json ./.alexa/config.json
	cp -f ~/.alexa/tokens.db ./.alexa/tokens.db
	-docker run --rm -h "`hostname`" -it -v `pwd`:/root alexacli/alexacli /bin/bash

clean:
	-rm -rf .eggs/
	-rm -rf build/
	-rm -rf dist/
	-rm -rf artifacts
	-rm -rf __pycache__
	-rm -rf alexatext/__pycache__
	-rm -rf *.egg-info/
	-rm -rf .pytest_cache
	-rm -rf .coverage
