.PHONY: test

install: clean
	python -m pip install -r requirements.txt
	python setup.py install --force

integtest:
	python alexa_cli/cli.py "what time is it?" --verbose

container:
	rm -rf alexa_cli/__pycache__
	docker build -t xebxeb/alexa-cli .

dockertest: container
	docker run --rm -it -v ~/.alexa:/root/.alexa xebxeb/alexa-cli alexa "what time is it?" -v

test:
	python -m pytest

clean:
	-rm -rf .eggs/
	-rm -rf __pycache__
	-rm -rf alexatext/__pycache__
	-rm -rf *.egg-info/
	-rm -rf .pytest_cache
