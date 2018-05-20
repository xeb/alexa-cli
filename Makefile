.PHONY: time

install: clean
	python setup.py install --force

test: clean
	pytest .

coverage: clean
	pytest --cov="alexatext" .

container:
	docker build -t xebxeb/alexa-cli -f Dockerfile .

debug: container
	-mkdir -p ./.alexa
	cp -f ~/.alexa/config.json ./.alexa/config.json
	cp -f ~/.alexa/tokens.db ./.alexa/tokens.db
	-docker run --rm -h "`hostname`" -it -v `pwd`:/root xebxeb/alexa-cli /bin/bash

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
