#!/usr/bin/env python

from alexa_cli.cli import main

class TestBasics():
    def test_options(self):
        response = main("what time is it", verbose=True, configure=False, output=None)
        assert response == None
