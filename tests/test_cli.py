import sys, os
path = os.path.join(os.path.dirname(__file__),os.pardir)
sys.path.append(path)
from alexatext.cli import request_from_alexa, init_config

def test_request_from_alexa():
	init_config(force_use_deepspeech=True)
	result = request_from_alexa("what is your favorite color", keep_artifacts=False, loc_artifacts=None, transcribe=True)
	assert(result == {'result': 'in for at his super pretty', 'success': True})

def test_request_from_alexa_with_google():
	init_config(reconfig=False, force_use_deepspeech=False)
	result = request_from_alexa("how far away is earth from mars", keep_artifacts=False, loc_artifacts=None, transcribe=True)
	assert(result == {'result': 'Mars distance of 64600000 Miles 104 million kilometers', 'success': True})