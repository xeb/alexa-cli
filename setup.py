from setuptools import setup
import sys
import os
import shutil

VERSION_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'VERSION')

setup(
    name='alexa',
    version=open(VERSION_PATH, 'r').read().strip(),
    license='MIT',
    install_requires=[
    ],
    entry_points={
        'console_scripts': [
            'alexa=alexa_cli.cli:entry'
        ]
    },
    packages=['alexa_cli'],
    setup_requires=['pytest-runner'],
    zip_safe=True
)
